//! `codemod-agent`: one `agent` operation over the process's own streams.
//!
//! stdin carries the request: one line of `OperationRequest` JSON, then,
//! only with `CODEMOD_AGENT_SECRETS=stdin`, `{"LLM_API_KEY": "..."}` (the key
//! never has to be in the launch environment). stdout carries the
//! `OperationCompletion` JSON and nothing else. Exit code: 0 completion
//! written, 2 unexpected arguments, 3 unreadable or malformed request
//! (including any other operation kind; a completion is still written), 4
//! runtime failure or stdout not writable.
//!
//! The response channel cannot be reached by what the agent runs: before any
//! thread or child exists, stdout is moved to a private close-on-exec
//! descriptor and fds 0 and 1 are pointed at `/dev/null`, so tool processes
//! (the builtin `bash`, MCP servers, the `claude`/`codex` CLIs) inherit
//! neither the request nor the response stream.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::process::ExitCode;

use orchestration_agent::{agent, parse_request, CompletionStatus, OperationCompletion};

/// Larger requests are refused (the prompt plus its JSON input).
const REQUEST_LIMIT: u64 = 64 * 1024 * 1024;

fn main() -> ExitCode {
    if std::env::args_os().len() > 1 {
        return ExitCode::from(2);
    }
    let mut input = Vec::new();
    let read = std::io::stdin()
        .lock()
        .take(REQUEST_LIMIT + 1)
        .read_to_end(&mut input);
    let Ok(mut response) = isolate_streams() else {
        return ExitCode::from(4);
    };
    let text = match read {
        Ok(_) if input.len() as u64 > REQUEST_LIMIT => {
            Err(format!("request is larger than {REQUEST_LIMIT} bytes"))
        }
        Ok(_) => String::from_utf8(input).map_err(|_| "request is not UTF-8".to_string()),
        Err(error) => Err(format!("failed to read request: {error}")),
    };
    let text = match text {
        Ok(text) => text,
        Err(message) => return write_error(&mut response, "", message),
    };
    let (request_text, secrets) = text.split_once('\n').unwrap_or((&text, ""));
    let request = match parse_request(request_text) {
        Ok(request) => request,
        Err(error) => {
            agent::scrub_process_env();
            return write_error(&mut response, &command_id_hint(request_text), error);
        }
    };
    let task = agent::Task::from_operation(&request.operation).expect("an agent operation");
    // A builtin agent's settings are read and the key removed from the
    // process environment before any thread exists: the agent's tool
    // processes inherit this environment. External backends never read the
    // key; it is only removed.
    let settings = if task.backend.is_builtin() {
        agent::take_process_settings(secrets.as_bytes())
    } else {
        agent::scrub_process_env();
        Err("external backends take no LLM settings".to_string())
    };
    let Ok(runtime) = tokio::runtime::Runtime::new() else {
        return ExitCode::from(4);
    };
    let completion = runtime.block_on(agent::run(
        &request.command_id,
        task,
        move || settings,
        std::env::current_dir(),
    ));
    match write_completion(&mut response, &completion) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(4),
    }
}

/// Move stdout to a private close-on-exec descriptor (the response channel)
/// and point fds 0 and 1 at `/dev/null`. Must run while single-threaded.
fn isolate_streams() -> std::io::Result<File> {
    // SAFETY: plain descriptor calls on this process's own fds; the duplicate
    // is owned by the returned `File` and nothing else uses it.
    unsafe {
        let private = libc::fcntl(libc::STDOUT_FILENO, libc::F_DUPFD_CLOEXEC, 3);
        if private < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let response = File::from(OwnedFd::from_raw_fd(private));
        let null = File::options().read(true).write(true).open("/dev/null")?;
        use std::os::fd::AsRawFd;
        for target in [libc::STDIN_FILENO, libc::STDOUT_FILENO] {
            if libc::dup2(null.as_raw_fd(), target) < 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(response)
    }
}

/// Best-effort `commandId` from a request that failed validation.
fn command_id_hint(text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| value.get("commandId")?.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn write_error(response: &mut File, command_id: &str, message: String) -> ExitCode {
    let completion =
        OperationCompletion::not_succeeded(command_id, CompletionStatus::Failed, message);
    // The exit code already reports the failure; a second write error is not recoverable.
    let _ = write_completion(response, &completion);
    ExitCode::from(3)
}

fn write_completion(response: &mut File, completion: &OperationCompletion) -> std::io::Result<()> {
    let text = serde_json::to_string(completion)?;
    response.write_all(text.as_bytes())?;
    response.flush()
}
