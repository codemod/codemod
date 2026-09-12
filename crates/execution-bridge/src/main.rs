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

use butterflow_execution_bridge::{execute, parse_request, CompletionStatus, OperationCompletion};
use butterflow_runners::direct_runner::DirectRunner;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [request_path, response_path] = args.as_slice() else {
        return ExitCode::from(2);
    };
    let response_path = Path::new(response_path);
    let text = match std::fs::read_to_string(request_path) {
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
    let Ok(runtime) = tokio::runtime::Runtime::new() else {
        return ExitCode::from(4);
    };
    let completion = runtime.block_on(execute(&DirectRunner::with_quiet(true), &request));
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

fn write_completion(path: &Path, completion: &OperationCompletion) -> std::io::Result<()> {
    std::fs::write(path, serde_json::to_string(completion)?)
}
