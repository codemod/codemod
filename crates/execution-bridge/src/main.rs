//! `butterflow-execution-bridge <request.json> <response.json>`
//!
//! Reads an `OperationRequest`, executes it, and writes an
//! `OperationCompletion` to the response file. Files are the protocol channel
//! because non-CLI crates must not write to the process streams. Problems are
//! reported through the exit code and, whenever the response path is known,
//! an error completion: 0 completion written, 2 wrong arguments, 3 unreadable
//! or malformed request, 4 runtime failure or response not writable.

use std::path::Path;
use std::process::ExitCode;

use butterflow_execution_bridge::{
    agent, execute, parse_request, CompletionStatus, OperationCompletion,
};
use butterflow_runners::direct_runner::DirectRunner;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [request_path, response_path] = args.as_slice() else {
        return ExitCode::from(2);
    };
    let response_path = Path::new(response_path);
    let text = match read_request(Path::new(request_path)) {
        Ok(text) => text,
        Err(error) => {
            return write_error(
                response_path,
                "",
                format!("failed to read request: {error}"),
            )
        }
    };
    let request = match parse_request(&text) {
        Ok(request) => request,
        Err(error) => return write_error(response_path, &command_id_hint(&text), error),
    };
    // A builtin agent's settings are read (the key from stdin when the host
    // sends it there), and the key removed from the process environment,
    // before any thread exists: the agent's tool processes inherit this
    // environment. External backends never read the key; it is only removed.
    let agent_task = agent::Task::from_operation(&request.operation);
    let agent_settings = agent_task.map(|task| {
        if task.backend.is_builtin() {
            agent::take_process_settings(std::io::stdin().lock())
        } else {
            agent::scrub_process_env();
            Err("external backends take no LLM settings".to_string())
        }
    });
    let Ok(runtime) = tokio::runtime::Runtime::new() else {
        return ExitCode::from(4);
    };
    let completion = match (agent_task, agent_settings) {
        (Some(task), Some(settings)) => runtime.block_on(agent::run(
            &request.command_id,
            task,
            move || settings,
            std::env::current_dir(),
        )),
        _ => runtime.block_on(execute(&DirectRunner::with_quiet(true), &request)),
    };
    match write_completion(response_path, &completion) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::from(4),
    }
}

/// Best-effort `commandId` from a request that failed validation.
fn command_id_hint(text: &str) -> String {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|value| value.get("commandId")?.as_str().map(str::to_string))
        .unwrap_or_default()
}

fn write_error(path: &Path, command_id: &str, message: String) -> ExitCode {
    let completion =
        OperationCompletion::not_succeeded(command_id, CompletionStatus::Failed, message);
    // The exit code already reports the failure; a second write error is not recoverable.
    let _ = write_completion(path, &completion);
    ExitCode::from(3)
}

/// The request must be a regular file, not a symlink to one.
fn read_request(path: &Path) -> std::io::Result<String> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "request is not a regular file",
        ));
    }
    std::fs::read_to_string(path)
}

/// The response is created, never opened: `create_new` (`O_CREAT | O_EXCL`)
/// fails if anything, including a symlink, already exists at the path, so a
/// planted link or file is never written through or overwritten.
fn write_completion(path: &Path, completion: &OperationCompletion) -> std::io::Result<()> {
    use std::io::Write;
    let text = serde_json::to_string(completion)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}
