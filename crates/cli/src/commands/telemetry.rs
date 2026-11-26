use std::ffi::OsStr;
use std::io::IsTerminal;

use anyhow::Result;
use clap::{Args, Subcommand};

use crate::auth::TokenStorage;
use crate::utils::interactive::is_interactive;

#[derive(Args, Debug)]
pub struct Command {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand, Debug)]
enum Action {
    /// Enable telemetry for this installation
    On,
    /// Disable telemetry for this installation
    Off,
    /// Show whether telemetry is enabled
    Status,
}

pub(crate) fn handler(args: &Command, invocation_opt_out: bool) -> Result<()> {
    let storage = TokenStorage::new()?;
    match args.action {
        Action::On => storage.set_telemetry_enabled(true)?,
        Action::Off => storage.set_telemetry_enabled(false)?,
        Action::Status => {}
    }
    let status = if storage.telemetry_disabled()? {
        "off"
    } else if invocation_opt_out {
        "off for this invocation (environment or --disable-analytics)"
    } else {
        "on"
    };
    println!("Telemetry is {status}.");
    Ok(())
}

pub(crate) fn maybe_print_notice() {
    if !is_interactive(false)
        || !std::io::stdout().is_terminal()
        || suppress_notice(std::env::args_os().skip(1))
    {
        return;
    }

    match TokenStorage::new().and_then(|storage| storage.claim_telemetry_notice()) {
        Ok(true) => eprintln!(
            "Codemod collects CLI usage (commands, versions, platform and run results), linked to your account when signed in. Turn it off with DO_NOT_TRACK=1 or codemod telemetry off."
        ),
        Ok(false) => {}
        Err(error) => log::debug!("Failed to record the telemetry notice: {error}"),
    }
}

// Check flags before dispatch, including implicit package runs. An end of
// options marker allows literal arguments that happen to have these names.
fn suppress_notice(args: impl IntoIterator<Item = impl AsRef<OsStr>>) -> bool {
    let mut format_value = false;
    for arg in args {
        let arg = arg.as_ref().to_string_lossy();
        if arg == "--" {
            break;
        }
        if (format_value && matches!(arg.as_ref(), "json" | "jsonl" | "yaml"))
            || matches!(arg.as_ref(), "--no-interactive" | "--control" | "--jsonl")
            || arg.starts_with("--control=")
            || matches!(arg.as_ref(), "--format=json" | "--format=jsonl" | "--format=yaml")
        {
            return true;
        }
        format_value = arg == "--format";
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppresses_notice_for_unattended_and_structured_flags() {
        for flags in [
            &["--no-interactive"][..],
            &["--control", "stdio"],
            &["--control=stdio"],
            &["--format", "jsonl"],
            &["--format=jsonl"],
            &["--format", "json"],
        ] {
            assert!(suppress_notice(flags));
        }
        assert!(!suppress_notice(["--format", "text", "--", "--no-interactive"]));
    }

    #[test]
    fn notice_and_preference_are_independent_and_persistent() {
        let dir = tempfile::tempdir().unwrap();
        let storage = TokenStorage::with_config_dir(dir.path().to_path_buf()).unwrap();
        assert!(!storage.telemetry_disabled().unwrap());
        assert!(storage.claim_telemetry_notice().unwrap());
        storage.set_telemetry_enabled(false).unwrap();
        storage.set_telemetry_enabled(false).unwrap();

        let next = TokenStorage::with_config_dir(dir.path().to_path_buf()).unwrap();
        assert!(!next.claim_telemetry_notice().unwrap());
        assert!(next.telemetry_disabled().unwrap());
        next.set_telemetry_enabled(true).unwrap();
        next.set_telemetry_enabled(true).unwrap();
        assert!(!next.telemetry_disabled().unwrap());
        assert!(!next.claim_telemetry_notice().unwrap());
    }
}
