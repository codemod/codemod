use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use uuid::Uuid;

use super::app::App;

pub fn resize_pty_and_parser(app: &mut App, rows: u16, cols: u16) {
    if app.terminal_size != (rows, cols) && rows > 0 && cols > 0 {
        app.terminal_size = (rows, cols);
        // Handle lock poisoning gracefully - if lock is poisoned, recover from it
        let mut parser = app
            .terminal_parser
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        parser.set_size(rows, cols);

        if let Some(master) = &mut app.pty_master {
            let _ = master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }
}

/// Run workflow resume command in a PTY (pseudo-terminal) for full interactivity
///
/// This spawns the command in a real PTY, allowing:
/// - Programs to detect they're running in a terminal
/// - Full color and formatting support
/// - Interactive prompts and user input
/// - Proper signal handling (Ctrl+C, etc.)
pub fn run_resume_command_in_terminal(
    app: &mut App,
    workflow_path: &Path,
    run_id: Uuid,
    task_ids: Option<Vec<Uuid>>,
    trigger_all: bool,
    target_path: Option<&Path>,
) -> Result<()> {
    // Get the current executable path
    let exe_path = std::env::current_exe().context("Failed to get current executable path")?;

    // Create PTY system
    let pty_system = NativePtySystem::default();

    // Get actual terminal size (fallback to app state if unavailable)
    let (rows, cols) = crossterm::terminal::size().unwrap_or(app.terminal_size);
    // Update app state with actual size
    app.terminal_size = (rows, cols);

    // Create PTY pair
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("Failed to open PTY")?;

    // Build command using portable-pty's CommandBuilder
    let mut cmd = CommandBuilder::new(&exe_path);
    cmd.arg("workflow");
    cmd.arg("resume");
    cmd.arg("--workflow");
    cmd.arg(workflow_path.to_string_lossy().as_ref());
    cmd.arg("--id");
    cmd.arg(run_id.to_string());
    // Note: We don't use --allow-dirty or --no-interactive so that prompts are shown

    // Add target path if available
    if let Some(target) = target_path {
        cmd.arg("--target");
        cmd.arg(target.to_string_lossy().as_ref());
    }

    if trigger_all {
        cmd.arg("--trigger-all");
    } else if let Some(ids) = task_ids {
        for task_id in ids {
            cmd.arg("--tasks_ids");
            cmd.arg(task_id.to_string());
        }
    }
    if app.allow_fs {
        cmd.arg("--allow-fs");
    }
    if app.allow_fetch {
        cmd.arg("--allow-fetch");
    }
    if app.allow_child_process {
        cmd.arg("--allow-child-process");
    }

    if app.dry_run {
        cmd.arg("--dry-run");
    }

    // Spawn child process in the PTY
    let _child = pair
        .slave
        .spawn_command(cmd)
        .context("Failed to spawn command in PTY")?;

    // Get writer for sending input to the PTY
    let writer = pair
        .master
        .take_writer()
        .context("Failed to get PTY writer")?;

    // Get reader for reading output from the PTY
    let mut reader = pair
        .master
        .try_clone_reader()
        .context("Failed to get PTY reader")?;

    // Store the writer for input
    app.pty_writer = Some(writer);
    // Store the master for resizing
    app.pty_master = Some(pair.master);

    // Reset the terminal parser for fresh output
    {
        let mut parser = app
            .terminal_parser
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *parser = vt100::Parser::new(rows, cols, 1000); // 1000 lines scrollback
    }

    // Mark PTY as running
    {
        let mut running = app
            .pty_running
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *running = true;
    }

    // Spawn a background thread to read PTY output and feed it to the parser
    // We use std::thread instead of tokio because portable-pty uses blocking I/O
    let parser_clone = app.terminal_parser.clone();
    let running_clone = app.pty_running.clone();

    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    // EOF - process exited
                    let mut running = running_clone
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    *running = false;
                    break;
                }
                Ok(n) => {
                    // Feed the raw bytes to the VT100 parser
                    // This handles all escape sequences, colors, cursor positioning, etc.
                    let mut parser = parser_clone
                        .write()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    parser.process(&buf[..n]);
                }
                Err(e) => {
                    // Error reading - log and exit
                    eprintln!("PTY read error: {}", e);
                    let mut running = running_clone
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    *running = false;
                    break;
                }
            }
        }
    });

    Ok(())
}
