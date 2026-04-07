#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use tempfile::TempDir;

type PtyInput<'a> = (Duration, &'a [u8], Option<&'a [&'a str]>);

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

const TUI_READY_PATTERNS: &[&str] = &["Workflow Runs", "Tasks", "Allow execution?", "Select agent"];
const APPROVAL_PATTERNS: &[&str] = &["Allow execution?", "capabilities", "Select agent"];

fn run_shell_in_pty(
    script: &str,
    cwd: &Path,
    xdg_home: &Path,
    input: &[u8],
    timeout: Duration,
) -> (String, portable_pty::ExitStatus) {
    run_shell_in_pty_with_inputs(script, cwd, xdg_home, &[(timeout, input, None)])
}

fn run_shell_in_pty_with_inputs(
    script: &str,
    cwd: &Path,
    xdg_home: &Path,
    inputs: &[PtyInput<'_>],
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

    // Single reader thread — avoids two-reader race where both readers compete
    // for the same PTY master bytes (causes blocking reads on Linux).
    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(256);
    let mut reader = pair.master.try_clone_reader().unwrap();
    thread::spawn(move || {
        let mut buffer = [0u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buffer[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let mut accumulated = String::new();

    {
        let mut writer = pair.master.take_writer().unwrap();
        for (wait_timeout, input, patterns) in inputs {
            let patterns_to_wait = patterns.unwrap_or(TUI_READY_PATTERNS);

            // Wait for pattern using channel recv_timeout so we never block
            // indefinitely (PTY reads block on Linux when no data is ready).
            let deadline = Instant::now() + *wait_timeout;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    eprintln!(
                        "Warning: timeout waiting for patterns {:?}\nAccumulated output:\n{}",
                        patterns_to_wait, accumulated
                    );
                    break;
                }
                match rx.recv_timeout(remaining.min(Duration::from_millis(100))) {
                    Ok(bytes) => {
                        if let Ok(chunk) = std::str::from_utf8(&bytes) {
                            accumulated.push_str(chunk);
                        }
                        if patterns_to_wait.iter().any(|p| accumulated.contains(p)) {
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        eprintln!(
                            "Warning: timeout waiting for patterns {:?}\nAccumulated output:\n{}",
                            patterns_to_wait, accumulated
                        );
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }

            writer.write_all(input).unwrap();
            writer.flush().unwrap();
        }
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("timed out waiting for PTY child to exit");
        }
        thread::sleep(Duration::from_millis(50));
    };

    drop(pair.master);

    // Drain remaining output from the reader thread (bounded to avoid hanging).
    let drain_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = drain_deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        match rx.recv_timeout(remaining.min(Duration::from_millis(100))) {
            Ok(bytes) => {
                if let Ok(chunk) = std::str::from_utf8(&bytes) {
                    accumulated.push_str(chunk);
                }
            }
            Err(_) => break,
        }
    }

    (accumulated, status)
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
        Duration::from_secs(5),
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
        &[3], // Ctrl+C
        Duration::from_secs(5),
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
        Duration::from_secs(5),
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
            (Duration::from_secs(5), b"n", Some(APPROVAL_PATTERNS)),
            (Duration::from_secs(3), b"q", None),
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
