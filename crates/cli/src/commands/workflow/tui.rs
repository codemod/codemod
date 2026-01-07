use crate::tui::app::App;
use crate::tui::pty::resize_pty_and_parser;
use crate::tui::render::ui;
pub use crate::tui::types::Command;
use crate::tui::types::Popup;
use std::{io, io::Stdout, time::Duration};

use anyhow::{Context, Result};
use butterflow_core::engine::Engine;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::engine::create_engine;

/// Run the TUI event loop
async fn run_tui(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    loop {
        // Refresh data if needed
        if app.should_refresh() {
            app.refresh().await?;
        }

        // Auto-dismiss status messages after 2 seconds (unless monitoring)
        if let Popup::StatusMessage(_, instant) = &app.popup {
            if instant.elapsed() > Duration::from_secs(2) && app.monitoring_task.is_none() {
                app.popup = Popup::None;
            }
        }

        // If monitoring a task or workflow, refresh more frequently to see logs and status
        if (app.monitoring_task.is_some() || app.monitoring_workflow.is_some())
            && app.last_refresh.elapsed() >= Duration::from_millis(200)
        {
            app.refresh().await?;
        }

        terminal.hide_cursor().context("Failed to hide cursor")?;

        // Render
        terminal.draw(|f| ui(f, app))?;

        // Handle events with timeout for periodic refresh
        let timeout = if app.refresh_interval.is_zero() {
            Duration::from_millis(33) // ~30 FPS for smooth animations
        } else {
            Duration::from_millis(33).min(app.refresh_interval)
        };

        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(key) => {
                    app.handle_input(key.code, key.modifiers).await?;
                }
                Event::Resize(cols, rows) => {
                    // Handle terminal resize events immediately
                    resize_pty_and_parser(app, rows, cols);
                }
                _ => {}
            }
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

/// Initialize and run the TUI
pub async fn handler(args: &Command) -> Result<()> {
    // Create a minimal engine first - we'll recreate it with proper workflow path when needed
    // For now, use current directory as placeholder
    let workflow_file_path = std::env::current_dir()?;
    let target_path = std::env::current_dir()?;

    // Create engine using create_engine like resume.rs
    let (_, config) = create_engine(
        workflow_file_path,
        target_path,
        false, // dry_run
        false, // allow_dirty - respect git checks
        std::collections::HashMap::new(),
        None,
        None,  // capabilities - will be resolved from workflow run when needed
        false, // no_interactive - TUI is interactive
    )?;

    let engine = Engine::with_workflow_run_config(config);

    let refresh_interval = if args.refresh_interval == 0 {
        Duration::ZERO
    } else {
        Duration::from_secs(args.refresh_interval)
    };

    let mut app = App::new(
        engine,
        args.limit,
        refresh_interval,
        args.dry_run,
        args.allow_fs,
        args.allow_fetch,
        args.allow_child_process,
    );

    // Setup terminal
    enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .context("Failed to setup terminal")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("Failed to create terminal")?;

    // Run the TUI
    let result = run_tui(&mut terminal, &mut app).await;

    // Restore terminal
    disable_raw_mode().context("Failed to disable raw mode")?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )
    .context("Failed to restore terminal")?;
    terminal.show_cursor().context("Failed to show cursor")?;

    result
}
