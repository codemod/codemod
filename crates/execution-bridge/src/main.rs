//! One-shot file protocol for the TypeScript orchestration prototype:
//!
//! ```text
//! butterflow-execution-bridge <request.json> <response.json>
//! ```
//!
//! Reads an `OperationRequest`, executes it through the existing
//! `DirectRunner`, and writes an `OperationCompletion` to the response file.
//! This binary never writes to stdout or stderr. Problems are reported through
//! the exit code and, whenever a response path is available, an error
//! completion in the response file.
//!
//! Exit codes: 0 completion written, 2 wrong arguments, 3 unreadable or
//! malformed request, 4 response could not be written or runtime failed.

use std::path::Path;
use std::process::ExitCode;

use butterflow_execution_bridge::{
    execute, parse_request, CompletionError, CompletionStatus, OperationCompletion,
    PROTOCOL_VERSION,
};
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
                3,
            )
        }
    };
    let request = match parse_request(&text) {
        Ok(request) => request,
        Err(error) => return write_error(response_path, &command_id_hint(&text), error, 3),
    };
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            return write_error(
                response_path,
                &request.command_id,
                format!("failed to start runtime: {error}"),
                4,
            )
        }
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

fn write_error(path: &Path, command_id: &str, message: String, code: u8) -> ExitCode {
    let completion = OperationCompletion {
        protocol_version: PROTOCOL_VERSION,
        command_id: command_id.to_string(),
        status: CompletionStatus::Failed,
        output: None,
        error: Some(CompletionError {
            message,
            exit_code: None,
            output: None,
        }),
    };
    // The exit code already reports the failure; a second write error is not recoverable.
    let _ = write_completion(path, &completion);
    ExitCode::from(code)
}

fn write_completion(path: &Path, completion: &OperationCompletion) -> std::io::Result<()> {
    let json = serde_json::to_string(completion)?;
    std::fs::write(path, json)
}
