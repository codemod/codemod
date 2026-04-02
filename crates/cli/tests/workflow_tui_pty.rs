#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::thread;
use std::time::Duration;

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tempfile::TempDir;

fn codemod_binary() -> &'static str {
    env!("CARGO_BIN_EXE_codemod")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn write_manual_workflow(path: &Path, auto_marker: &str) {
    let workflow = format!(
        r#"version: "1"
nodes:
  - id: prepare
    name: Prepare
    type: automatic
    runtime:
      type: direct
    steps:
      - id: prepare-step
        name: Prepare Step
        run: printf '{auto_marker}\n'

  - id: gate
    name: Gate
    type: automatic
    depends_on:
      - prepare
    trigger:
      type: manual
    runtime:
      type: direct
    steps:
      - id: gate-step
        name: Gate Step
        run: printf 'manual gate\n'
"#
    );

    fs::write(path, workflow).unwrap();
}

fn run_shell_in_pty(
    script: &str,
    cwd: &Path,
    xdg_home: &Path,
    input: &[u8],
    delay: Duration,
) -> (String, portable_pty::ExitStatus) {
    run_shell_in_pty_with_inputs(script, cwd, xdg_home, &[(delay, input)])
}

fn run_shell_in_pty_with_inputs(
    script: &str,
    cwd: &Path,
    xdg_home: &Path,
    inputs: &[(Duration, &[u8])],
) -> (String, portable_pty::ExitStatus) {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 40,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();

    let mut command = CommandBuilder::new("/bin/sh");
    command.arg("-lc");
    command.arg(script);
    command.cwd(cwd);
    command.env("XDG_DATA_HOME", xdg_home);
    command.env("XDG_CACHE_HOME", xdg_home.join("cache"));
    command.env("TERM", "xterm-256color");
    command.env("NO_COLOR", "1");

    let mut child = pair.slave.spawn_command(command).unwrap();
    drop(pair.slave);

    let (tx, rx) = std::sync::mpsc::channel();
    let mut reader = pair.master.try_clone_reader().unwrap();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        tx.send(String::from_utf8_lossy(&bytes).into_owned())
            .unwrap();
    });

    {
        let mut writer = pair.master.take_writer().unwrap();
        for (delay, input) in inputs {
            thread::sleep(*delay);
            writer.write_all(input).unwrap();
            writer.flush().unwrap();

            if cfg!(target_os = "macos") {
                thread::sleep(Duration::from_millis(50));
            }
        }
    }

    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("timed out waiting for PTY child to exit");
        }
        thread::sleep(Duration::from_millis(50));
    };
    drop(pair.master);
    let output = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    (output, status)
}

fn build_shell_script(command: String) -> String {
    format!("{command}; status=$?; stty -a; printf '__AFTER__:%s\\n' \"$status\"")
}

#[test]
fn workflow_tui_quits_with_q_and_restores_terminal() {
    let temp_dir = TempDir::new().unwrap();
    let script = build_shell_script(format!(
        "{} --disable-analytics workflow tui --limit 1",
        shell_quote(codemod_binary())
    ));

    let (output, status) = run_shell_in_pty(
        &script,
        temp_dir.path(),
        temp_dir.path(),
        b"q",
        Duration::from_millis(2500),
    );

    assert!(status.success());
    assert!(output.contains("__AFTER__:0"));
    assert!(output.contains("icanon"));
}

#[test]
fn workflow_tui_exits_after_ctrl_c() {
    let temp_dir = TempDir::new().unwrap();
    let script = format!(
        "{} --disable-analytics workflow tui --limit 1",
        shell_quote(codemod_binary())
    );

    let (output, status) = run_shell_in_pty(
        &script,
        temp_dir.path(),
        temp_dir.path(),
        &[3],
        Duration::from_millis(1200),
    );

    assert!(
        status.success() || status.signal().is_some(),
        "expected the TUI to exit cleanly or via interrupt\nstatus={status:?}\n{output}"
    );
}

#[test]
fn workflow_run_auto_enters_tui_and_suppresses_task_stdout() {
    let temp_dir = TempDir::new().unwrap();
    let workflow_path = temp_dir.path().join("workflow.yaml");
    write_manual_workflow(&workflow_path, "AUTO_LOG_MARKER");

    let command = format!(
        "{} --disable-analytics workflow run -w {} -t {}",
        shell_quote(codemod_binary()),
        shell_quote(workflow_path.to_string_lossy().as_ref()),
        shell_quote(temp_dir.path().to_string_lossy().as_ref()),
    );
    let script = build_shell_script(command);

    let (output, status) = run_shell_in_pty(
        &script,
        temp_dir.path(),
        temp_dir.path(),
        b"q",
        Duration::from_millis(3000),
    );

    assert!(
        output.contains("__AFTER__:"),
        "expected post-run marker in shell output, status={status:?}\n{output}"
    );
    assert!(
        output.contains("icanon"),
        "expected restored terminal mode, status={status:?}\n{output}"
    );
    assert!(
        !output.contains("AUTO_LOG_MARKER"),
        "task stdout leaked into the TUI session, status={status:?}\n{output}"
    );
    assert!(
        !output.contains("Prepare Step") && !output.contains("Gate Step"),
        "engine step headers leaked into the TUI session, status={status:?}\n{output}"
    );
}

#[test]
fn workflow_run_decline_shell_approval_keeps_terminal_clean() {
    let temp_dir = TempDir::new().unwrap();
    let workflow_path = temp_dir.path().join("workflow.yaml");
    write_manual_workflow(&workflow_path, "AUTO_LOG_MARKER");

    let command = format!(
        "{} --disable-analytics workflow run -w {} -t {}",
        shell_quote(codemod_binary()),
        shell_quote(workflow_path.to_string_lossy().as_ref()),
        shell_quote(temp_dir.path().to_string_lossy().as_ref()),
    );
    let script = build_shell_script(command);

    let (output, status) = run_shell_in_pty_with_inputs(
        &script,
        temp_dir.path(),
        temp_dir.path(),
        &[
            (Duration::from_millis(3000), b"n"),
            (Duration::from_millis(1000), b"q"),
        ],
    );

    assert!(
        output.contains("__AFTER__:"),
        "expected post-run marker in shell output, status={status:?}\n{output}"
    );
    assert!(
        output.contains("icanon"),
        "expected restored terminal mode, status={status:?}\n{output}"
    );
    assert!(
        !output.contains("ERROR butterflow_core::engine"),
        "tracing error output leaked into the TUI session, status={status:?}\n{output}"
    );
    assert!(
        !output.contains("Task execution failed"),
        "engine failure text leaked into the TUI session, status={status:?}\n{output}"
    );
}
