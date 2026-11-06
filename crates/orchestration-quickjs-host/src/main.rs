//! `orchestration-quickjs-host run <bundle.mjs> [--target <dir>] [--input <json>]
//! [--history <file>] [--timings] [--json]`
//!
//! Runs a workflow bundle built by `codemod-workflow build --runtime=quickjs`
//! in one QuickJS context and prints the final output as JSON on stdout, as
//! `codemod-workflow run` does. `--history <file>` resumes from the history
//! in the file when it exists and writes the run's history back to it
//! (whether or not the run succeeded), so a failed run can be resumed.
//! `--timings` (or `CODEMOD_QUICKJS_TIMINGS=1`) prints a timer breakdown as
//! one `timings: {...}` JSON line on stderr. Workflow and transform console
//! output goes to stderr. `--json` prints `{ ok, output | error }` on stdout
//! instead (what the Node provider parses).
//!
//! Exit codes: 0 completed, 1 the workflow failed or was cancelled, 2 usage,
//! 3 the host could not run the bundle.
//!
//! `orchestration-quickjs-host select` runs the native file selection the
//! context's `Host.selectFiles` / `Host.fs.readSources` use on a JSON array of
//! `{ request, read? }` jobs from stdin and prints one result per job
//! (`select.rs`); the differential walker test drives it.

use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use orchestration_quickjs_host::{run, select, HostOptions};

const USAGE: &str = "usage: orchestration-quickjs-host run <bundle.mjs> [--target <directory>] [--input <json>] [--history <file>] [--timings] [--json]\n       orchestration-quickjs-host select < jobs.json";

fn main() -> ExitCode {
    let started = Instant::now();
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
        Some("select") => {
            let mut input = String::new();
            if let Err(error) = std::io::stdin().read_to_string(&mut input) {
                eprintln!("failed to read stdin: {error}");
                return ExitCode::from(2);
            }
            return match select::run_jobs(&input) {
                Ok(output) => {
                    println!("{output}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("{error}");
                    ExitCode::from(2)
                }
            };
        }
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
    let mut timings = std::env::var("CODEMOD_QUICKJS_TIMINGS").is_ok_and(|value| value == "1");
    let mut json = false;
    while let Some(flag) = args.next() {
        if flag == "--timings" {
            timings = true;
            continue;
        }
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
            timings,
        },
        started,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("{error}");
            // Skip tearing down the QuickJS runtime: the process is done.
            std::process::exit(3);
        }
    };
    let result = outcome.result;
    if let (Some(path), Some(history)) = (&history_file, result.get("history")) {
        if let Err(error) = std::fs::write(path, history.to_string()) {
            eprintln!("failed to write history {}: {error}", path.display());
        }
    }
    if let Some(timings) = result.get("timings") {
        eprintln!("timings: {timings}");
    }
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
        std::process::exit(if ok { 0 } else { 1 });
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
    std::process::exit(code);
}
