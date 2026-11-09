//! Benchmark timings for `codemod workflow run -w workflow.ts` (the
//! `timings` feature): with `CODEMOD_TIMINGS_FILE` set, one file whose
//! phases cover the run without double counting; unset, nothing is written.
//!
//! cargo test --release -p codemod --features timings --test typescript_timings
#![cfg(feature = "timings")]

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::Value;
use support::{output, runtime_sources, Scratch};

const FILES: usize = 3;

fn repository() -> BTreeMap<String, String> {
    (0..FILES)
        .map(|i| {
            (
                format!("src/f{i}.ts"),
                format!("import {{ it }} from \"vitest\";\nconsole.log(\"f{i}\", {i});\n"),
            )
        })
        .collect()
}

/// Two benchmark transforms over every file.
fn workflow(scratch: &Scratch) -> std::path::PathBuf {
    let transforms =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../benchmarks/runtimes/transforms");
    let path = scratch.path("workflow.ts");
    fs::write(
        &path,
        format!(
            r#"import {{ jssg, sequence }} from {index:?};
import consoleToLogger from {a:?};
import renameVitestImport from {b:?};
const files = {{ language: "typescript", include: ["**/*.ts"] }} as const;
const a = jssg({{ name: "console-to-logger", ...files, transform: consoleToLogger }});
const b = jssg({{ name: "rename-vitest-import", ...files, transform: renameVitestImport }});
export default sequence(a(), b());
"#,
            index = runtime_sources().join("index.ts").canonicalize().unwrap(),
            a = transforms
                .join("console-to-logger.ts")
                .canonicalize()
                .unwrap(),
            b = transforms
                .join("rename-vitest-import.ts")
                .canonicalize()
                .unwrap(),
        ),
    )
    .unwrap();
    path
}

/// The timings file of one run.
fn timings() -> Value {
    let scratch = Scratch::with(&repository());
    let file = scratch.path("run.timings.json");
    let mut command = scratch.command(&workflow(&scratch), &[]);
    command
        .env("CODEMOD_TIMINGS_FILE", &file)
        .env_remove("CODEMOD_TIMINGS_ROOT_PID")
        .env_remove("BENCH_T0_NS");
    output(&command.output().unwrap());
    serde_json::from_str(&fs::read_to_string(file).unwrap()).unwrap()
}

fn count(timings: &Value, phase: &str) -> u64 {
    timings["phases"][phase]["count"].as_u64().unwrap_or(0)
}

#[test]
fn a_typescript_run_is_labelled_ts_quickjs() {
    assert_eq!(timings()["arm"], "ts-quickjs");
}

#[test]
fn every_phase_of_a_run_is_recorded() {
    let timings = timings();
    let missing: Vec<&str> = [
        "boot", "plan", "select", "read", "setup", "parse", "conflict", "commit",
    ]
    .into_iter()
    .filter(|&phase| count(&timings, phase) == 0)
    .collect();
    assert_eq!(missing, Vec::<&str>::new());
}

#[test]
fn each_transform_call_is_counted_once() {
    assert_eq!(count(&timings(), "transform"), 2 * FILES as u64);
}

#[test]
fn the_phases_cover_most_of_the_wall_without_double_counting() {
    let timings = timings();
    let phases: u64 = timings["phases"]
        .as_object()
        .unwrap()
        .values()
        .map(|phase| phase["ns"].as_u64().unwrap())
        .sum();
    let covered = phases as f64 / timings["wall_ns"].as_f64().unwrap();
    assert!((0.8..=1.01).contains(&covered), "{covered}");
}

#[test]
fn nothing_is_written_without_the_timings_file_variable() {
    let scratch = Scratch::with(&repository());
    let cwd = scratch.path("cwd");
    fs::create_dir_all(&cwd).unwrap();
    let mut command = scratch.command(&workflow(&scratch), &[]);
    command.current_dir(&cwd).env_remove("CODEMOD_TIMINGS_FILE");
    output(&command.output().unwrap());
    assert_eq!(
        (fs::read_dir(&cwd).unwrap().count(), scratch.files().len()),
        (0, FILES)
    );
}
