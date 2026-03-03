pub mod actions;
pub mod app;
pub mod event;
pub mod screens;

use std::io::{self, stdout, Write};

use anyhow::Result;
use butterflow_core::engine::Engine;
use crossterm::{
    event::{KeyCode, KeyModifiers},
    style::{self, Stylize},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{backend::CrosstermBackend, Terminal};
use uuid::Uuid;

use app::{App, Screen};
use event::{AppEvent, EventHandler};

use self::actions::Action;

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
    fn redirect() -> (std::fs::File, Self) {
        use std::os::unix::io::{AsRawFd, FromRawFd};

        unsafe {
            let real_stdout = io::stdout().as_raw_fd();
            let real_stderr = io::stderr().as_raw_fd();

            let saved_stdout_fd = libc_dup(real_stdout);
            let saved_stderr_fd = libc_dup(real_stderr);
            assert!(saved_stdout_fd >= 0 && saved_stderr_fd >= 0);

            // Dup for ratatui to write to the real terminal
            let tty_fd = libc_dup(real_stdout);
            assert!(tty_fd >= 0);
            let tty_file = std::fs::File::from_raw_fd(tty_fd);

            let devnull_fd = libc_open(b"/dev/null\0".as_ptr(), O_WRONLY);
            assert!(devnull_fd >= 0);

            libc_dup2(devnull_fd, 1);
            libc_dup2(devnull_fd, 2);

            (
                tty_file,
                Self {
                    saved_stdout_fd,
                    saved_stderr_fd,
                    devnull_fd,
                },
            )
        }
    }

    /// Temporarily restore real stdout/stderr (e.g. for inline log viewer).
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
        restore_terminal();
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
    let (tty_file, stdio_guard) = StdioGuard::redirect();

    let backend = CrosstermBackend::new(tty_file);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // Initial data fetch
    app.refresh().await?;

    // Event loop
    let mut events = EventHandler::new(std::time::Duration::from_millis(500));

    let result = run_event_loop(&mut app, &mut terminal, &mut events, &stdio_guard).await;

    // Always cleanup terminal, even if the loop errored
    restore_terminal();
    stdio_guard.restore();
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
                            // Restore real stdout/stderr for the log viewer
                            stdio_guard.pause_redirect();
                            view_logs_inline(terminal, events, &app.engine, wf_id, task_id).await?;
                            stdio_guard.resume_redirect();
                        }
                        Action::TriggerTask(wf_id, task_id) => {
                            if let Err(e) = app.engine.resume_workflow(wf_id, vec![task_id]).await {
                                app.status_message = Some(format!("Failed to trigger task: {e}"));
                            }
                            app.refresh().await?;
                        }
                        Action::TriggerAll(wf_id) => {
                            if let Err(e) = app.engine.trigger_all(wf_id).await {
                                app.status_message = Some(format!("Failed to trigger all: {e}"));
                            }
                            app.refresh().await?;
                        }
                        Action::RetryFailed(wf_id, task_id) => {
                            if let Err(e) = app.engine.resume_workflow(wf_id, vec![task_id]).await {
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

/// Returns true if the key should exit the log viewer (q, Esc, or Ctrl+C)
fn is_exit_key(key: &crossterm::event::KeyEvent) -> bool {
    key.code == KeyCode::Char('q') || key.code == KeyCode::Esc || is_ctrl_c(key)
}

/// View task logs inline.
///
/// We stay in the alternate screen (so the user's original terminal buffer is
/// preserved) but disable raw mode so the terminal handles line wrapping and
/// scrolling naturally.  The alt-screen content is cleared before printing so
/// consecutive log views don't stack.
async fn view_logs_inline(
    terminal: &mut Terminal<CrosstermBackend<std::fs::File>>,
    events: &mut EventHandler,
    engine: &Engine,
    workflow_run_id: Uuid,
    task_id: Uuid,
) -> Result<()> {
    // Stay in the alternate screen -- only disable raw mode so println! works
    // with normal line discipline (wrapping, scrolling, etc.)
    disable_raw_mode()?;

    // Clear the alternate screen so previous log views don't linger.
    print!("\x1b[2J\x1b[H");
    io::stdout().flush()?;

    // Get tasks and find the specific one
    let tasks = engine.get_tasks(workflow_run_id).await?;
    let task = tasks.iter().find(|t| t.id == task_id);

    if let Some(task) = task {
        println!();
        println!(
            "  {} {} {}",
            style::style("task").bold(),
            style::style(&task.node_id).with(style::Color::Rgb {
                r: 110,
                g: 140,
                b: 255
            }),
            style::style(format!("({})", &task.id.to_string()[..8])).with(style::Color::Rgb {
                r: 100,
                g: 100,
                b: 110
            }),
        );

        let status_color = match task.status {
            butterflow_models::TaskStatus::Completed => style::Color::Rgb {
                r: 80,
                g: 200,
                b: 120,
            },
            butterflow_models::TaskStatus::Failed => style::Color::Rgb {
                r: 240,
                g: 80,
                b: 80,
            },
            butterflow_models::TaskStatus::Running => style::Color::Rgb {
                r: 240,
                g: 200,
                b: 60,
            },
            butterflow_models::TaskStatus::AwaitingTrigger => style::Color::Rgb {
                r: 80,
                g: 210,
                b: 220,
            },
            _ => style::Color::Rgb {
                r: 100,
                g: 100,
                b: 110,
            },
        };

        println!(
            "  {} {}\n",
            style::style("\u{25cf}").with(status_color),
            style::style(format!("{:?}", task.status)).with(status_color),
        );

        if task.logs.is_empty() {
            println!(
                "  {}",
                style::style("(no logs yet)").with(style::Color::Rgb {
                    r: 100,
                    g: 100,
                    b: 110
                })
            );
        } else {
            for line in &task.logs {
                println!("  {line}");
            }
        }

        if let Some(error) = &task.error {
            println!();
            println!(
                "  {} {}",
                style::style("error:")
                    .with(style::Color::Rgb {
                        r: 240,
                        g: 80,
                        b: 80
                    })
                    .bold(),
                error,
            );
        }

        println!();
    } else {
        println!(
            "\n  {}",
            style::style(format!("Task {task_id} not found.")).with(style::Color::Rgb {
                r: 240,
                g: 80,
                b: 80
            })
        );
    }

    // If task is still running, poll for new logs
    if task
        .map(|t| t.status == butterflow_models::TaskStatus::Running)
        .unwrap_or(false)
    {
        println!(
            "  {}",
            style::style("Streaming logs\u{2026} press q/Esc to return").with(style::Color::Rgb {
                r: 130,
                g: 130,
                b: 145
            })
        );
        let mut last_log_count = task.map(|t| t.logs.len()).unwrap_or(0);

        enable_raw_mode()?;

        loop {
            let evt = events.next().await?;
            match evt {
                AppEvent::Key(key) => {
                    if is_exit_key(&key) {
                        break;
                    }
                }
                AppEvent::Tick => {
                    if let Ok(tasks) = engine.get_tasks(workflow_run_id).await {
                        if let Some(t) = tasks.iter().find(|t| t.id == task_id) {
                            if t.logs.len() > last_log_count {
                                disable_raw_mode()?;
                                for line in &t.logs[last_log_count..] {
                                    println!("  {line}");
                                }
                                io::stdout().flush()?;
                                enable_raw_mode()?;
                                last_log_count = t.logs.len();
                            }
                            if t.status != butterflow_models::TaskStatus::Running {
                                disable_raw_mode()?;
                                let done_color = match t.status {
                                    butterflow_models::TaskStatus::Completed => style::Color::Rgb {
                                        r: 80,
                                        g: 200,
                                        b: 120,
                                    },
                                    butterflow_models::TaskStatus::Failed => style::Color::Rgb {
                                        r: 240,
                                        g: 80,
                                        b: 80,
                                    },
                                    _ => style::Color::Rgb {
                                        r: 130,
                                        g: 130,
                                        b: 145,
                                    },
                                };
                                println!(
                                    "\n  {} {}",
                                    style::style("\u{25cf}").with(done_color),
                                    style::style(format!("Task finished: {:?}", t.status))
                                        .with(done_color),
                                );
                                enable_raw_mode()?;
                                break;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        disable_raw_mode()?;
    }

    println!(
        "  {}",
        style::style("Press any key to return\u{2026}").with(style::Color::Rgb {
            r: 130,
            g: 130,
            b: 145
        })
    );
    io::stdout().flush()?;

    // Wait for keypress
    enable_raw_mode()?;
    loop {
        let evt = events.next().await?;
        if matches!(evt, AppEvent::Key(_)) {
            break;
        }
    }

    // Re-enable raw mode and let ratatui redraw on the next frame.
    // We're still in the alt screen -- terminal.clear() just tells ratatui
    // to do a full repaint.
    terminal.clear()?;

    Ok(())
}
