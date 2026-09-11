//! `butterflow-execution-bridge`: the Rust side of the TypeScript
//! orchestration prototype, in two modes.
//!
//! One-shot file protocol (exec):
//!
//! ```text
//! butterflow-execution-bridge <request.json> <response.json>
//! ```
//!
//! Reads an `OperationRequest`, executes it, and writes an
//! `OperationCompletion` to the response file. Problems are reported through
//! the exit code and, whenever a response path is available, an error
//! completion in the response file. Exit codes: 0 completion written, 2
//! wrong arguments, 3 unreadable or malformed request, 4 response could not
//! be written or runtime failed.
//!
//! JSSG worker (JSONL):
//!
//! ```text
//! butterflow-execution-bridge --jssg-worker
//! ```
//!
//! Reads worker messages from stdin and answers on stdout, one JSON object
//! per line, until `close` or EOF (see `worker.rs`). The pipes are the
//! protocol channel owned by the host that spawned the worker; nothing else
//! is ever written to them, and the sandbox's `console` goes to runtime
//! events, not to the process streams. Exit codes: 0 closed or EOF, 3
//! malformed message or protocol misuse (an `error` line was written first),
//! 4 I/O failure.

use std::io::{BufReader, BufWriter};
use std::path::Path;
use std::process::ExitCode;

use butterflow_execution_bridge::{
    execute, parse_request, worker::run_worker, CompletionStatus, OperationCompletion,
};
use butterflow_runners::direct_runner::DirectRunner;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(_) => return ExitCode::from(4),
    };
    match args.as_slice() {
        [flag] if flag == "--jssg-worker" => {
            let stdin = std::io::stdin();
            let stdout = std::io::stdout();
            let code = run_worker(
                runtime.handle(),
                BufReader::new(stdin.lock()),
                BufWriter::new(stdout.lock()),
            );
            ExitCode::from(code)
        }
        [request_path, response_path] => one_shot(&runtime, request_path, Path::new(response_path)),
        _ => ExitCode::from(2),
    }
}

fn one_shot(
    runtime: &tokio::runtime::Runtime,
    request_path: &str,
    response_path: &Path,
) -> ExitCode {
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
    let completion =
        OperationCompletion::not_succeeded(command_id, CompletionStatus::Failed, message);
    // The exit code already reports the failure; a second write error is not recoverable.
    let _ = write_completion(path, &completion);
    ExitCode::from(code)
}

fn write_completion(path: &Path, completion: &OperationCompletion) -> std::io::Result<()> {
    let json = serde_json::to_string(completion)?;
    std::fs::write(path, json)
}
