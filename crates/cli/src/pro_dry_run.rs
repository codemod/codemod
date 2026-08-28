use std::io::{self, IsTerminal};

use butterflow_core::config::WorkflowRunConfig;
use console::style;

pub(crate) enum ProDryRunReason<'a> {
    TopLevelCodemod,
    BundledChild { source: &'a str },
}

pub(crate) fn notify_pro_dry_run_required(reason: ProDryRunReason<'_>, no_interactive: bool) {
    if no_interactive {
        return;
    }

    let notice = match reason {
        ProDryRunReason::TopLevelCodemod => {
            "This is a Pro codemod. Preview changes and insights for free with no login or code sharing. \
             Applying changes and advanced insights requires a Pro plan and signing in. \
             Learn more: codemod.com/contact."
                .to_string()
        }
        ProDryRunReason::BundledChild { source } => {
            format!(
                "This bundle includes a Pro codemod ({source}). Preview changes and insights for free with no login or code sharing. \
                 Applying changes and advanced insights requires a Pro plan and signing in. \
                 Learn more: codemod.com/contact."
            )
        }
    };
    let notice = style(notice).yellow();

    // Only block for a keypress when both stdin and stdout are attached
    // to a terminal. Otherwise the notice (or the "press any key" line)
    // would be hidden from the user; route it to stderr and continue.
    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        println!("{notice}");
        println!("{}", style("Press any key to proceed.").dim());
        wait_for_any_key();
    } else {
        eprintln!("{notice}");
    }
}

pub(crate) fn apply_pro_dry_run_execution_settings(cfg: &mut WorkflowRunConfig) {
    cfg.execution.auto_trigger_manual_steps = true;
    cfg.execution.skip_shard_steps = true;
    cfg.execution.skip_state_writes = true;
    cfg.execution.flatten_matrix_tasks = true;
}

/// Block until the user presses any key. Falls back to a no-op when either
/// stdin or stdout isn't a terminal (e.g. piped input or redirected output)
/// or when raw mode can't be enabled. Callers must ensure any prompt they want
/// the user to read has already been printed to a stream the user can see.
fn wait_for_any_key() {
    use crossterm::event::{read, Event, KeyEventKind};
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};

    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return;
    }
    if enable_raw_mode().is_err() {
        return;
    }
    loop {
        match read() {
            Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => break,
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    let _ = disable_raw_mode();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_pro_dry_run_execution_settings_enables_preview_only_mode() {
        let mut config = WorkflowRunConfig::default();

        apply_pro_dry_run_execution_settings(&mut config);

        assert!(config.execution.auto_trigger_manual_steps);
        assert!(config.execution.skip_shard_steps);
        assert!(config.execution.skip_state_writes);
        assert!(config.execution.flatten_matrix_tasks);
    }
}
