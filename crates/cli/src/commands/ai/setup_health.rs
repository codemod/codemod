//! `codemod ai doctor` and `codemod ai uninstall`.

use crate::commands::harness_adapter::{
    doctor, installed_harnesses, plan_uninstall, resolve_install_scope, CheckStatus, DoctorOptions,
    DoctorReport, Harness, InstallScope, KeptItem, OutputFormat, RemovedItem, UninstallPlan,
    DEFAULT_MCP_PROBE_TIMEOUT,
};
use crate::commands::output::{exit_adapter_error, format_output_path};
use anyhow::{bail, Result};
use clap::Args;
use inquire::Confirm;
use serde::Serialize;
use std::io::IsTerminal;

#[derive(Args, Debug)]
pub struct DoctorCommand {
    /// Coding agent to check (default: every agent Codemod is set up for here)
    #[arg(long, value_enum, default_value_t = Harness::Auto)]
    harness: Harness,
    /// Check only the setup for this project
    #[arg(long, conflicts_with = "user")]
    project: bool,
    /// Check only the setup for all your projects
    #[arg(long, conflicts_with = "project")]
    user: bool,
    /// Don't start the Codemod MCP server to check that it answers
    #[arg(long)]
    skip_mcp_start: bool,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Logs)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
pub struct UninstallCommand {
    /// Coding agent to remove Codemod from (default: the one set up here)
    #[arg(long, value_enum, default_value_t = Harness::Auto)]
    harness: Harness,
    /// Remove the setup for this project (default)
    #[arg(long, conflicts_with = "user")]
    project: bool,
    /// Remove the setup for all your projects
    #[arg(long, conflicts_with = "project")]
    user: bool,
    /// Show what would be removed without changing anything
    #[arg(long)]
    dry_run: bool,
    /// Remove without asking (needed when there's no terminal to ask in)
    #[arg(long)]
    yes: bool,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Logs)]
    format: OutputFormat,
}

fn requested_scope(project: bool, user: bool) -> Option<InstallScope> {
    if user {
        Some(InstallScope::User)
    } else if project {
        Some(InstallScope::Project)
    } else {
        None
    }
}

fn print_structured(value: &impl Serialize, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Yaml => print!("{}", serde_yaml::to_string(value)?),
        _ => println!("{}", serde_json::to_string_pretty(value)?),
    }
    Ok(())
}

fn is_structured(format: OutputFormat) -> bool {
    matches!(format, OutputFormat::Json | OutputFormat::Yaml)
}

pub async fn handle_doctor(command: &DoctorCommand) -> Result<()> {
    let scope = requested_scope(command.project, command.user);
    let mcp_probe_timeout = (!command.skip_mcp_start).then_some(DEFAULT_MCP_PROBE_TIMEOUT);
    if mcp_probe_timeout.is_some() && !is_structured(command.format) {
        eprintln!("Checking Codemod's setup; starting the MCP server can take a minute the first time npx downloads it.");
    }
    let options = DoctorOptions {
        harness: command.harness,
        scope,
        mcp_probe_timeout,
    };
    let reports = tokio::task::spawn_blocking(move || doctor(&options))
        .await?
        .unwrap_or_else(|error| exit_adapter_error(error, command.format));
    let ok = !reports.is_empty() && !reports.iter().any(DoctorReport::failed);

    if is_structured(command.format) {
        #[derive(Serialize)]
        struct DoctorOutput<'a> {
            ok: bool,
            setups: &'a [DoctorReport],
        }
        print_structured(
            &DoctorOutput {
                ok,
                setups: &reports,
            },
            command.format,
        )?;
    } else {
        print!("{}", render_doctor_logs(&reports));
    }
    if !ok {
        std::process::exit(1);
    }
    Ok(())
}

pub(crate) fn render_doctor_logs(reports: &[DoctorReport]) -> String {
    let mut out = String::from("\n");
    if reports.is_empty() {
        out.push_str(
            "  No Codemod setup found here. Run `npx codemod ai` to set Codemod up for your coding agent.\n\n",
        );
        return out;
    }
    for report in reports {
        let place = match report.scope {
            "user" => "for all your projects",
            _ => "in this project",
        };
        out.push_str(&format!("  Codemod for {} {place}\n", report.agent));
        for check in &report.checks {
            let mark = match check.status {
                CheckStatus::Pass => "✓",
                CheckStatus::Warn => "⚠",
                CheckStatus::Fail => "✗",
            };
            out.push_str(&format!("  {mark} {}: {}\n", check.name, check.detail));
            if let Some(fix) = &check.fix {
                out.push_str(&format!("    Fix: {fix}\n"));
            }
        }
        out.push('\n');
    }
    let count = |status: CheckStatus| {
        reports
            .iter()
            .flat_map(|report| &report.checks)
            .filter(|check| check.status == status)
            .count()
    };
    let plural =
        |count: usize, word: &str| format!("{count} {word}{}", if count == 1 { "" } else { "s" });
    let summary = match (count(CheckStatus::Fail), count(CheckStatus::Warn)) {
        (0, 0) => "Everything works.".to_string(),
        (0, warnings) => format!("Works, with {} to check.", plural(warnings, "thing")),
        (failures, _) => format!(
            "{} found. Run the Fix lines above, then `npx codemod ai doctor` again.",
            plural(failures, "problem")
        ),
    };
    out.push_str(&format!("  {summary}\n\n"));
    out
}

pub async fn handle_uninstall(command: &UninstallCommand) -> Result<()> {
    let scope = resolve_install_scope(command.project, command.user)
        .unwrap_or_else(|error| exit_adapter_error(error, command.format));
    let place = match scope {
        InstallScope::Project => "in this project",
        InstallScope::User => "for all your projects",
    };
    let harness = match command.harness {
        Harness::Auto => {
            let found = installed_harnesses(scope)
                .unwrap_or_else(|error| exit_adapter_error(error, command.format));
            match found.as_slice() {
                [] => {
                    println!("\n  Nothing to remove: Codemod isn't set up for any coding agent {place}.\n");
                    return Ok(());
                }
                [harness] => *harness,
                several => {
                    let names = several
                        .iter()
                        .map(|harness| harness.as_str())
                        .collect::<Vec<_>>()
                        .join("|");
                    bail!(
                        "Codemod is set up for {} {place}. Choose one: `npx codemod ai uninstall --harness <{names}>`",
                        several
                            .iter()
                            .map(|harness| harness.display_name())
                            .collect::<Vec<_>>()
                            .join(" and ")
                    );
                }
            }
        }
        harness => harness,
    };
    let plan = plan_uninstall(harness, scope)
        .unwrap_or_else(|error| exit_adapter_error(error, command.format));

    let interactive = std::io::stdin().is_terminal() && std::io::stdout().is_terminal();
    let mut needs_yes = false;
    let applied = if plan.is_empty() || command.dry_run {
        false
    } else if command.yes {
        true
    } else if interactive && !is_structured(command.format) {
        print!("{}", render_uninstall_logs(&plan, false));
        Confirm::new("Remove these?").with_default(false).prompt()?
    } else {
        needs_yes = true;
        false
    };
    if applied {
        plan.apply()
            .unwrap_or_else(|error| exit_adapter_error(error, command.format));
    }

    if is_structured(command.format) {
        #[derive(Serialize)]
        struct UninstallOutput<'a> {
            ok: bool,
            harness: &'a str,
            scope: &'a str,
            dry_run: bool,
            removed: &'a [RemovedItem],
            kept: &'a [KeptItem],
        }
        print_structured(
            &UninstallOutput {
                ok: !needs_yes,
                harness: harness.as_str(),
                scope: scope.as_str(),
                dry_run: !applied,
                removed: &plan.removed,
                kept: &plan.kept,
            },
            command.format,
        )?;
    } else if needs_yes {
        print!("{}", render_uninstall_logs(&plan, false));
        println!("  Nothing was removed. Add --yes to remove these without a prompt.\n");
    } else if command.dry_run && !plan.is_empty() {
        print!("{}", render_uninstall_logs(&plan, false));
        println!("  Dry run: nothing was changed.\n");
    } else if applied || plan.is_empty() {
        print!("{}", render_uninstall_logs(&plan, applied));
    } else {
        println!("\n  Nothing was removed.\n");
    }
    if needs_yes {
        std::process::exit(1);
    }
    Ok(())
}

pub(crate) fn render_uninstall_logs(plan: &UninstallPlan, applied: bool) -> String {
    let agent = plan.harness.display_name();
    let place = match plan.scope {
        InstallScope::Project => "in this project",
        InstallScope::User => "for all your projects",
    };
    let mut out = String::from("\n");
    if plan.is_empty() && plan.kept.is_empty() {
        out.push_str(&format!(
            "  Nothing to remove: Codemod isn't set up for {agent} {place}.\n\n"
        ));
        return out;
    }
    let (heading, mark) = if applied {
        (format!("Removed Codemod from {agent} {place}."), "✓")
    } else {
        (format!("This removes Codemod from {agent} {place}:"), "-")
    };
    out.push_str(&format!("  {heading}\n"));
    for item in &plan.removed {
        out.push_str(&format!(
            "  {mark} {} ({})\n",
            item.what,
            format_output_path(&item.path)
        ));
    }
    for item in &plan.kept {
        out.push_str(&format!(
            "  • Kept {} ({}): {}\n",
            item.what,
            format_output_path(&item.path),
            item.reason
        ));
    }
    if applied {
        out.push_str(&format!(
            "\n  Restart {agent} so it stops loading Codemod. Set it up again any time with `npx codemod ai`.\n"
        ));
    }
    out.push('\n');
    out
}
