pub mod actions;
pub mod app;
pub mod event;
pub mod screens;

use std::collections::HashSet;
use std::io::{self, stdout};
use std::process::Command as ProcessCommand;

use anyhow::Result;
use butterflow_core::engine::Engine;
use butterflow_models::WorkflowRun;
use codemod_llrt_capabilities::types::LlrtSupportedModules;
use crossterm::{
    event::{KeyCode, KeyModifiers},
    style::Stylize,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{backend::CrosstermBackend, Terminal};
use uuid::Uuid;

use app::{App, Screen};
use event::{AppEvent, EventHandler};

use self::actions::Action;

fn append_tui_debug_log(message: impl AsRef<str>) {
    let path = std::env::var("CODEMOD_TUI_DEBUG_LOG")
        .unwrap_or_else(|_| "/tmp/codemod-tui-debug.log".to_string());
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write;
        let _ = writeln!(
            file,
            "[{}] {}",
            chrono::Utc::now().to_rfc3339(),
            message.as_ref()
        );
    }
}

fn normalize_target_path(path: std::path::PathBuf) -> Result<std::path::PathBuf> {
    let absolute = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    };

    if absolute.exists() {
        Ok(absolute.canonicalize()?)
    } else {
        Ok(absolute)
    }
}

/// Run the TUI starting at the run list
pub async fn run_tui(engine: Engine, limit: usize) -> Result<()> {
    let app = App::new(engine, limit);
    run_tui_loop(app).await
}

/// Run the TUI starting at the task list for a specific workflow run
pub async fn run_tui_for_run(engine: Engine, workflow_run_id: Uuid) -> Result<()> {
    let app = App::new_for_run(engine, workflow_run_id);
    run_tui_loop(app).await
}

/// Restore the terminal to a sane state (idempotent, safe to call multiple times)
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = stdout().execute(LeaveAlternateScreen);
}

/// Returns true if the key event is Ctrl+C
fn is_ctrl_c(key: &crossterm::event::KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

// Thin syscall wrappers — avoids pulling in the `libc` crate.
extern "C" {
    #[link_name = "dup"]
    fn libc_dup(fd: i32) -> i32;
    #[link_name = "dup2"]
    fn libc_dup2(src: i32, dst: i32) -> i32;
    #[link_name = "open"]
    fn libc_open(path: *const u8, flags: i32) -> i32;
    #[link_name = "close"]
    fn libc_close(fd: i32) -> i32;
}

const O_WRONLY: i32 = 1;

/// Manages redirection of stdout/stderr to /dev/null while the TUI is active.
/// Keeps saved copies of the real fds so they can be temporarily restored
/// (e.g. for the inline log viewer) and permanently restored on TUI exit.
struct StdioGuard {
    /// Dup of the original stdout fd
    saved_stdout_fd: i32,
    /// Dup of the original stderr fd
    saved_stderr_fd: i32,
    /// Fd pointing to /dev/null (kept open for re-redirecting)
    devnull_fd: i32,
}

impl StdioGuard {
    /// Save real stdout/stderr, then redirect fd 1 & 2 to /dev/null.
    /// Returns the guard and a `File` handle to the real tty for ratatui.
    fn redirect() -> Result<(std::fs::File, Self)> {
        use std::os::unix::io::{AsRawFd, FromRawFd};

        unsafe {
            let real_stdout = io::stdout().as_raw_fd();
            let real_stderr = io::stderr().as_raw_fd();

            let saved_stdout_fd = libc_dup(real_stdout);
            let saved_stderr_fd = libc_dup(real_stderr);
            if saved_stdout_fd < 0 || saved_stderr_fd < 0 {
                anyhow::bail!("Failed to dup stdout/stderr file descriptors");
            }

            // Dup for ratatui to write to the real terminal
            let tty_fd = libc_dup(real_stdout);
            if tty_fd < 0 {
                anyhow::bail!("Failed to dup tty file descriptor");
            }
            let tty_file = std::fs::File::from_raw_fd(tty_fd);

            let devnull_fd = libc_open(b"/dev/null\0".as_ptr(), O_WRONLY);
            if devnull_fd < 0 {
                anyhow::bail!("Failed to open /dev/null");
            }

            libc_dup2(devnull_fd, 1);
            libc_dup2(devnull_fd, 2);

            Ok((
                tty_file,
                Self {
                    saved_stdout_fd,
                    saved_stderr_fd,
                    devnull_fd,
                },
            ))
        }
    }

    /// Temporarily restore real stdout/stderr (e.g. for child process).
    fn pause_redirect(&self) {
        unsafe {
            libc_dup2(self.saved_stdout_fd, 1);
            libc_dup2(self.saved_stderr_fd, 2);
        }
    }

    /// Re-redirect stdout/stderr back to /dev/null after a pause.
    fn resume_redirect(&self) {
        unsafe {
            libc_dup2(self.devnull_fd, 1);
            libc_dup2(self.devnull_fd, 2);
        }
    }

    /// Permanently restore real stdout/stderr and clean up.
    fn restore(self) {
        unsafe {
            libc_dup2(self.saved_stdout_fd, 1);
            libc_dup2(self.saved_stderr_fd, 2);
            libc_close(self.saved_stdout_fd);
            libc_close(self.saved_stderr_fd);
            libc_close(self.devnull_fd);
        }
    }
}

async fn run_tui_loop(mut app: App) -> Result<()> {
    // Install a panic hook that restores the terminal so a panic doesn't
    // leave the user stuck in raw mode / alternate screen.
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Write terminal-restore escape sequences directly to /dev/tty so they
        // reach the real terminal even if stdout/stderr are redirected to /dev/null.
        let _ = disable_raw_mode();
        if let Ok(mut tty) = std::fs::OpenOptions::new().write(true).open("/dev/tty") {
            use std::io::Write;
            // LeaveAlternateScreen + show cursor
            let _ = tty.write_all(b"\x1b[?1049l\x1b[?25h");
            let _ = tty.flush();
        }
        original_hook(info);
    }));

    // Suppress all log output while TUI is active
    let prev_log_level = log::max_level();
    log::set_max_level(log::LevelFilter::Off);

    // Setup terminal — must happen BEFORE we redirect fds so that
    // EnterAlternateScreen goes to the real terminal.
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;

    // Now redirect stdout (fd 1) and stderr (fd 2) to /dev/null so that
    // *nothing* — println!, eprintln!, log macros, child-process output,
    // JS runtime errors — can corrupt the ratatui alternate screen.
    // We get back a File handle to the real tty for ratatui to write to.
    let (tty_file, stdio_guard) = StdioGuard::redirect()?;

    let backend = CrosstermBackend::new(tty_file);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // Initial data fetch
    app.refresh().await?;

    // Event loop
    let mut events = EventHandler::new(std::time::Duration::from_millis(500));

    let result = run_event_loop(&mut app, &mut terminal, &mut events, &stdio_guard).await;

    // Always cleanup: restore stdio FIRST so LeaveAlternateScreen reaches the
    // real terminal instead of /dev/null.
    stdio_guard.restore();
    restore_terminal();
    log::set_max_level(prev_log_level);

    result
}

async fn run_event_loop(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<std::fs::File>>,
    events: &mut EventHandler,
    stdio_guard: &StdioGuard,
) -> Result<()> {
    let mut needs_redraw = true;

    loop {
        // Only draw when something changed
        if needs_redraw {
            terminal.draw(|f| {
                let area = f.area();
                match &app.screen {
                    Screen::RunList => {
                        screens::run_list::render(
                            f,
                            area,
                            &app.workflow_runs,
                            &mut app.run_list_state,
                        );
                    }
                    Screen::TaskList { .. } => {
                        screens::task_list::render(
                            f,
                            area,
                            app.current_workflow_run.as_ref(),
                            &app.tasks,
                            &mut app.task_list_state,
                        );
                    }
                    Screen::Settings { .. } => {
                        screens::settings::render(
                            f,
                            area,
                            app.current_workflow_run.as_ref(),
                            app.settings_cursor,
                            app.engine.is_dry_run(),
                            app.engine.get_capabilities(),
                        );
                    }
                }
            })?;
            needs_redraw = false;
        }

        // Handle events
        let event = events.next().await?;
        match event {
            AppEvent::Key(key) => {
                // Ctrl+C: restore terminal and force-exit the process.
                // This avoids waiting for background `spawn_blocking` tasks
                // (workflow execution) to finish during tokio runtime shutdown.
                if is_ctrl_c(&key) {
                    stdio_guard.pause_redirect();
                    restore_terminal();
                    std::process::exit(0);
                }

                // Any keypress triggers a redraw (cursor movement, etc.)
                needs_redraw = true;

                if let Some(action) = app.handle_key(key) {
                    match action {
                        Action::Quit => break,
                        Action::NavigateToTaskList(id) => {
                            app.navigate_to_task_list(id);
                            app.refresh().await?;
                        }
                        Action::NavigateToRunList => {
                            app.navigate_to_run_list();
                            app.refresh().await?;
                        }
                        Action::ViewLogs(wf_id, task_id) => {
                            events.pause();
                            stdio_guard.pause_redirect();
                            let result = view_logs_inline(&app.engine, wf_id, task_id).await;
                            restore_tui(terminal, stdio_guard, events).await?;
                            if let Err(e) = result {
                                app.status_message = Some(format!("Failed to view logs: {e}"));
                            }
                            app.refresh().await?;
                        }
                        Action::TriggerTask(wf_id, task_id) => {
                            events.pause();
                            stdio_guard.pause_redirect();
                            let result = app
                                .current_workflow_run
                                .as_ref()
                                .ok_or_else(|| anyhow::anyhow!("Workflow run not loaded"))
                                .and_then(|workflow_run| {
                                    run_tasks_via_child_process(
                                        &app.engine,
                                        workflow_run,
                                        wf_id,
                                        &[task_id],
                                    )
                                });
                            restore_tui(terminal, stdio_guard, events).await?;
                            if let Err(e) = result {
                                app.status_message = Some(format!("Failed to trigger task: {e}"));
                            }
                            app.refresh().await?;
                        }
                        Action::TriggerAll(wf_id) => {
                            events.pause();
                            stdio_guard.pause_redirect();
                            let result = app
                                .current_workflow_run
                                .as_ref()
                                .ok_or_else(|| anyhow::anyhow!("Workflow run not loaded"))
                                .and_then(|workflow_run| {
                                    run_tasks_via_child_process(
                                        &app.engine,
                                        workflow_run,
                                        wf_id,
                                        &[],
                                    )
                                });
                            restore_tui(terminal, stdio_guard, events).await?;
                            if let Err(e) = result {
                                app.status_message = Some(format!("Failed to trigger all: {e}"));
                            }
                            app.refresh().await?;
                        }
                        Action::RetryFailed(wf_id, task_id) => {
                            events.pause();
                            stdio_guard.pause_redirect();
                            let result = app
                                .current_workflow_run
                                .as_ref()
                                .ok_or_else(|| anyhow::anyhow!("Workflow run not loaded"))
                                .and_then(|workflow_run| {
                                    run_tasks_via_child_process(
                                        &app.engine,
                                        workflow_run,
                                        wf_id,
                                        &[task_id],
                                    )
                                });
                            restore_tui(terminal, stdio_guard, events).await?;
                            if let Err(e) = result {
                                app.status_message = Some(format!("Failed to retry task: {e}"));
                            }
                            app.refresh().await?;
                        }
                        Action::NavigateToSettings(wf_id) => {
                            app.navigate_to_settings(wf_id);
                            app.refresh().await?;
                        }
                        Action::NavigateBackFromSettings => {
                            app.navigate_back_from_settings();
                            app.refresh().await?;
                        }
                        Action::CancelWorkflow(wf_id) => {
                            if let Err(e) = app.engine.cancel_workflow(wf_id).await {
                                app.status_message =
                                    Some(format!("Failed to cancel workflow: {e}"));
                            }
                            app.refresh().await?;
                        }
                    }
                }
            }
            AppEvent::Tick => {
                if app.refresh().await? {
                    needs_redraw = true;
                }
            }
            AppEvent::Resize(_, _) => {
                needs_redraw = true;
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

/// Restore the TUI after a passthrough view: re-redirect stdio, re-enter
/// alternate screen, enable raw mode, resume the event handler, and force
/// a full terminal redraw so the user sees the task list immediately.
async fn restore_tui(
    terminal: &mut Terminal<CrosstermBackend<std::fs::File>>,
    stdio_guard: &StdioGuard,
    events: &mut EventHandler,
) -> Result<()> {
    // Ensure clean terminal state — child process may have left raw mode on.
    let _ = disable_raw_mode();

    // Re-redirect stdout/stderr to /dev/null BEFORE entering the alternate
    // screen — otherwise EnterAlternateScreen would go to the real terminal
    // and the ratatui backend (which writes to the tty_file fd) would be
    // out of sync.
    stdio_guard.resume_redirect();
    enable_raw_mode()?;

    // Write EnterAlternateScreen to the tty file via the ratatui backend
    // (not stdout, which now goes to /dev/null).
    use std::io::Write;
    let tty = terminal.backend_mut();
    crossterm::queue!(tty, EnterAlternateScreen)?;
    tty.flush()?;

    terminal.clear()?;
    events.resume();
    Ok(())
}

/// Resume workflow tasks via a child `codemod workflow resume` process so the
/// resumed execution gets a clean interactive terminal.
fn run_tasks_via_child_process(
    engine: &Engine,
    workflow_run: &WorkflowRun,
    workflow_run_id: Uuid,
    task_ids: &[Uuid],
) -> Result<()> {
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    let workflow_source = resolve_workflow_source(engine, workflow_run)?;
    let target_path = normalize_target_path(
        workflow_run
            .target_path
            .clone()
            .unwrap_or_else(|| engine.get_target_path()),
    )?;
    let current_executable = std::env::current_exe()?;

    let mut cmd = ProcessCommand::new(current_executable);
    cmd.arg("workflow")
        .arg("resume")
        .arg("--workflow")
        .arg(&workflow_source)
        .arg("--id")
        .arg(workflow_run_id.to_string())
        .arg("--target")
        .arg(&target_path)
        .arg("--allow-dirty")
        .arg("--exit-when-triggered-tasks-finish")
        .current_dir(&target_path);

    if engine.is_dry_run() {
        cmd.arg("--dry-run");
    }

    append_capability_flags(&mut cmd, workflow_run.capabilities.as_ref());

    if task_ids.is_empty() {
        cmd.arg("--trigger-all");
    } else {
        for task_id in task_ids {
            cmd.arg("--tasks_ids").arg(task_id.to_string());
        }
    }

    append_tui_debug_log(format!("launching resume subprocess: {:?}", cmd));
    eprintln!("[tui-debug] launching resume subprocess: {:?}", cmd);
    let status = cmd.status()?;
    append_tui_debug_log(format!("resume subprocess exited with status: {status}"));
    eprintln!("[tui-debug] resume subprocess exited with status: {status}");
    if !status.success() {
        anyhow::bail!("resume command exited with status {status}");
    }
    wait_for_any_key("\nTask is done.\n\nPress any key to return to the tasks list...")?;
    Ok(())
}

fn wait_for_any_key(message: &str) -> Result<()> {
    use std::io::Write;

    println!("{message}");
    std::io::stdout().flush()?;

    enable_raw_mode()?;
    loop {
        if matches!(crossterm::event::read()?, crossterm::event::Event::Key(_)) {
            break;
        }
    }
    disable_raw_mode()?;

    Ok(())
}

/// View task logs inline (standalone, used by the ViewLogs action).
///
/// Leaves the alternate screen, prints stored logs, returns.
async fn view_logs_inline(engine: &Engine, workflow_run_id: Uuid, task_id: Uuid) -> Result<()> {
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    if let Ok(tasks) = engine.get_tasks(workflow_run_id).await {
        if let Some(task) = tasks.iter().find(|t| t.id == task_id) {
            println!(
                "\n  {} {}",
                crossterm::style::style("task").bold(),
                crossterm::style::style(&task.node_id).with(crossterm::style::Color::Rgb {
                    r: 110,
                    g: 140,
                    b: 255,
                }),
            );
            println!(
                "  {} {:?}\n",
                crossterm::style::style("\u{25cf}").with(match task.status {
                    butterflow_models::TaskStatus::Completed => crossterm::style::Color::Rgb {
                        r: 80,
                        g: 200,
                        b: 120,
                    },
                    butterflow_models::TaskStatus::Failed => crossterm::style::Color::Rgb {
                        r: 240,
                        g: 80,
                        b: 80,
                    },
                    _ => crossterm::style::Color::Rgb {
                        r: 240,
                        g: 200,
                        b: 60,
                    },
                }),
                task.status,
            );

            if task.logs.is_empty() {
                println!(
                    "  {}",
                    crossterm::style::style("(no logs)").with(crossterm::style::Color::Rgb {
                        r: 100,
                        g: 100,
                        b: 110,
                    })
                );
            } else {
                for line in &task.logs {
                    println!("  {line}");
                }
            }

            if let Some(error) = &task.error {
                println!(
                    "\n  {} {}",
                    crossterm::style::style("error:")
                        .with(crossterm::style::Color::Rgb {
                            r: 240,
                            g: 80,
                            b: 80,
                        })
                        .bold(),
                    error,
                );
            }
        } else {
            println!("\n  Task {task_id} not found.");
        }
    }

    // Wait for any key before returning to TUI
    println!(
        "\n  {}",
        crossterm::style::style("Press Enter to return\u{2026}").with(
            crossterm::style::Color::Rgb {
                r: 130,
                g: 130,
                b: 145,
            }
        )
    );
    tokio::task::spawn_blocking(|| {
        let mut buf = String::new();
        let _ = std::io::stdin().read_line(&mut buf);
    })
    .await?;

    Ok(())
}

fn resolve_workflow_source(
    engine: &Engine,
    workflow_run: &WorkflowRun,
) -> Result<std::path::PathBuf> {
    if let Some(bundle_path) = workflow_run
        .bundle_path
        .as_ref()
        .filter(|path| path.exists())
    {
        return Ok(bundle_path.clone());
    }

    let workflow_file_path = engine.get_workflow_file_path();
    if workflow_file_path.exists() {
        return Ok(workflow_file_path);
    }

    anyhow::bail!(
        "Unable to determine workflow source for run {}",
        workflow_run.id
    );
}

fn append_capability_flags(
    cmd: &mut ProcessCommand,
    capabilities: Option<&HashSet<LlrtSupportedModules>>,
) {
    let Some(capabilities) = capabilities else {
        return;
    };

    if capabilities.contains(&LlrtSupportedModules::Fs) {
        cmd.arg("--allow-fs");
    }
    if capabilities.contains(&LlrtSupportedModules::Fetch) {
        cmd.arg("--allow-fetch");
    }
    if capabilities.contains(&LlrtSupportedModules::ChildProcess) {
        cmd.arg("--allow-child-process");
    }
}
