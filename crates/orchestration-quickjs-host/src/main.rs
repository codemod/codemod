//! `orchestration-quickjs-host run <bundle.mjs> [--target <dir>] [--input <json>]
//! [--history <file>] [--json]`
//!
//! Runs a workflow bundle built by `codemod-workflow build --runtime=quickjs`
//! in one QuickJS context and prints the final output as JSON on stdout, as
//! `codemod-workflow run` does. `--history <file>` resumes from the history
//! in the file when it exists and writes the run's history back to it
//! (whether or not the run succeeded), so a failed run can be resumed.
//! Workflow and transform console output goes to stderr. `--json` prints
//! `{ ok, output | error }` on stdout instead (what the Node provider parses).
//!
//! Benchmark timings: built with the `timings` feature and run with
//! `CODEMOD_TIMINGS_FILE` set, the host writes one `quickjs-host` timings
//! file (`crates/bench-timings`) with the bundle's JS-side phases merged in.
//!
//! Exit codes: 0 completed, 1 the workflow failed or was cancelled, 2 usage,
//! 3 the host could not run the bundle.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use orchestration_quickjs_host::{record_js_timings, run, HostOptions};

const USAGE: &str = "usage: orchestration-quickjs-host run <bundle.mjs> [--target <directory>] [--input <json>] [--history <file>] [--json]";

fn main() -> ExitCode {
    let started = Instant::now();
    bench_timings::init("quickjs-host");
    let code = run_main(started);
    bench_timings::finish();
    code
}

/// Exit now (the runtime and any still-pending JS futures are not torn
/// down), writing the timings file first.
fn exit(code: i32) -> ! {
    bench_timings::finish();
    std::process::exit(code);
}

fn run_main(started: Instant) -> ExitCode {
    let mut args = std::env::args()
        .skip(1)
        .flat_map(|arg| match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => {
                vec![flag.to_string(), value.to_string()]
            }
            _ => vec![arg],
        });
    match args.next().as_deref() {
        Some("run") => {}
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    }
    let Some(bundle) = args.next().filter(|arg| !arg.starts_with('-')) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let mut target = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut input = None;
    let mut history_file: Option<PathBuf> = None;
    let mut json = false;
    while let Some(flag) = args.next() {
        if flag == "--json" {
            json = true;
            continue;
        }
        let Some(value) = args.next() else {
            eprintln!("missing value for {flag}\n{USAGE}");
            return ExitCode::from(2);
        };
        match flag.as_str() {
            "--target" => target = PathBuf::from(value),
            "--input" => input = Some(value),
            "--history" => history_file = Some(PathBuf::from(value)),
            _ => {
                eprintln!("unknown option: {flag}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let history = match &history_file {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => Some(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                eprintln!("failed to read history {}: {error}", path.display());
                return ExitCode::from(3);
            }
        },
        None => None,
    };

    let outcome = match run(
        HostOptions {
            bundle: PathBuf::from(bundle),
            target,
            input,
            history,
        },
        started,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("{error}");
            // Skip tearing down the QuickJS runtime: the process is done.
            exit(3);
        }
    };
    let result = outcome.result;
    if let (Some(path), Some(history)) = (&history_file, result.get("history")) {
        if let Err(error) = std::fs::write(path, history.to_string()) {
            eprintln!("failed to write history {}: {error}", path.display());
        }
    }
    record_js_timings(&result);
    let ok = result.get("ok").and_then(|ok| ok.as_bool()) == Some(true);
    if json {
        // Machine form for `codemod-workflow run --runtime=quickjs`: `{ ok, output | error }`.
        let mut summary = serde_json::Map::new();
        for key in ["ok", "output", "error"] {
            if let Some(value) = result.get(key) {
                summary.insert(key.to_string(), value.clone());
            }
        }
        println!("{}", serde_json::Value::Object(summary));
        exit(if ok { 0 } else { 1 });
    }
    let code = if ok {
        let output = result
            .get("output")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        println!(
            "{}",
            serde_json::to_string_pretty(&output).unwrap_or_else(|_| "null".to_string())
        );
        0
    } else {
        let message = result
            .get("error")
            .and_then(|error| error.as_str())
            .unwrap_or("the workflow failed");
        eprintln!("{message}");
        1
    };
    // The runtime (and any still-pending JS futures) is not torn down: exit now.
    exit(code);
}
