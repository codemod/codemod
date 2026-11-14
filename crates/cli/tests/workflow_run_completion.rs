//! `codemod workflow run` must return as soon as the in-process workflow
//! executor finishes, instead of waiting out the status poll interval, while
//! runs that pause on a manual trigger still pause and can be resumed.

use std::path::Path;
use std::process::{Command, Output};
use std::sync::Once;
use std::time::{Duration, Instant};

/// Whole-process bound (debug binary, startup included). Generous for a loaded
/// CI machine, but below the 2 s the wait loop used to sleep when it first saw
/// a running workflow.
const FAST_EXIT: Duration = Duration::from_millis(1500);

/// Upper bound for the wait loop itself, as printed by the CLI in
/// "Workflow completed in X.Xs" (excludes process startup).
const FAST_WAIT_SECS: f64 = 0.3;

static WARM_BINARY: Once = Once::new();

/// Run `codemod` from `cwd` (shell steps inherit the process cwd).
fn codemod(home: &Path, cwd: &Path, args: &[&str]) -> (Output, Duration) {
    // The first exec of a freshly linked binary can take seconds on macOS;
    // keep that out of the timed runs.
    WARM_BINARY.call_once(|| {
        let _ = Command::new(env!("CARGO_BIN_EXE_codemod"))
            .arg("--version")
            .output();
    });
    let started = Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_codemod"))
        .current_dir(cwd)
        .arg("--disable-analytics")
        .args(args)
        // The local state adapter lives under the platform data dir; keep it
        // inside the test's temp dir.
        .env("HOME", home)
        .env("XDG_DATA_HOME", home.join("data"))
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run codemod");
    (output, started.elapsed())
}

fn describe(output: &Output) -> String {
    format!(
        "status: {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn workflow_run_id(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("Workflow started "))
        .map(|id| id.trim().to_string())
        .unwrap_or_else(|| panic!("no run id in output\n{}", describe(output)))
}

/// Seconds from "Workflow completed in X.Xs".
fn completed_wait_secs(output: &Output) -> f64 {
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .find_map(|line| line.trim().strip_prefix("Workflow completed in "))
        .and_then(|rest| rest.trim().strip_suffix('s'))
        .and_then(|secs| secs.parse().ok())
        .unwrap_or_else(|| panic!("no completion time in output\n{}", describe(output)))
}

fn write_workflow(dir: &Path, body: &str) -> String {
    let path = dir.join("workflow.yaml");
    std::fs::write(&path, body).unwrap();
    path.to_str().unwrap().to_string()
}

const TRIVIAL_WORKFLOW: &str = r#"version: "1"
nodes:
  - id: touch
    name: Touch
    type: automatic
    steps:
      - name: touch
        run: echo done > touched.txt
"#;

const MANUAL_WORKFLOW: &str = r#"version: "1"
nodes:
  - id: first
    name: First
    type: automatic
    steps:
      - name: first
        run: echo first > first.txt
  - id: gated
    name: Gated
    type: automatic
    depends_on: [first]
    trigger:
      type: manual
    steps:
      - name: gated
        run: echo gated > gated.txt
"#;

#[test]
fn trivial_workflow_exits_without_waiting_for_the_poll_interval() {
    let home = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let workflow = write_workflow(home.path(), TRIVIAL_WORKFLOW);
    let target_arg = target.path().to_str().unwrap();
    let args = [
        "workflow",
        "run",
        "-w",
        &workflow,
        "--target",
        target_arg,
        "--allow-dirty",
        "--no-interactive",
    ];

    // Warm-up run so binary load and first-run state setup aren't timed.
    let (output, _) = codemod(home.path(), target.path(), &args);
    assert!(output.status.success(), "{}", describe(&output));

    std::fs::remove_file(target.path().join("touched.txt")).unwrap();
    let (output, elapsed) = codemod(home.path(), target.path(), &args);
    assert!(output.status.success(), "{}", describe(&output));
    assert_eq!(
        std::fs::read_to_string(target.path().join("touched.txt"))
            .unwrap()
            .trim(),
        "done"
    );
    let wait_secs = completed_wait_secs(&output);
    assert!(
        wait_secs < FAST_WAIT_SECS,
        "wait loop took {wait_secs}s, expected < {FAST_WAIT_SECS}s\n{}",
        describe(&output)
    );
    assert!(
        elapsed < FAST_EXIT,
        "workflow run took {elapsed:?}, expected < {FAST_EXIT:?}\n{}",
        describe(&output)
    );
}

#[test]
fn manual_trigger_still_pauses_and_resumes() {
    let home = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let workflow = write_workflow(home.path(), MANUAL_WORKFLOW);
    let target_arg = target.path().to_str().unwrap();

    let (output, elapsed) = codemod(
        home.path(),
        target.path(),
        &[
            "workflow",
            "run",
            "-w",
            &workflow,
            "--target",
            target_arg,
            "--allow-dirty",
            "--no-interactive",
        ],
    );
    assert!(output.status.success(), "{}", describe(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("Workflow completed"),
        "{}",
        describe(&output)
    );
    assert!(target.path().join("first.txt").exists());
    assert!(
        !target.path().join("gated.txt").exists(),
        "gated step ran without its trigger"
    );
    assert!(
        elapsed < FAST_EXIT,
        "pausing took {elapsed:?}\n{}",
        describe(&output)
    );
    let run_id = workflow_run_id(&output);

    let (status, _) = codemod(
        home.path(),
        target.path(),
        &["workflow", "status", "-i", &run_id],
    );
    assert!(status.status.success(), "{}", describe(&status));
    assert!(
        String::from_utf8_lossy(&status.stdout).contains("AwaitingTrigger"),
        "{}",
        describe(&status)
    );

    let (output, elapsed) = codemod(
        home.path(),
        target.path(),
        &[
            "workflow",
            "resume",
            "-w",
            &workflow,
            "-i",
            &run_id,
            "--trigger-all",
            "--target",
            target_arg,
            "--allow-dirty",
            "--no-interactive",
        ],
    );
    assert!(output.status.success(), "{}", describe(&output));
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("Workflow completed successfully"),
        "{}",
        describe(&output)
    );
    assert_eq!(
        std::fs::read_to_string(target.path().join("gated.txt"))
            .unwrap()
            .trim(),
        "gated"
    );
    assert!(
        elapsed < FAST_EXIT,
        "resume took {elapsed:?}\n{}",
        describe(&output)
    );
}
