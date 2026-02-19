use crate::commands::harness_adapter::{
    resolve_adapter, resolve_install_scope, Harness, InstallRequest, OutputFormat,
};
use anyhow::{bail, Result};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct Command {
    #[command(subcommand)]
    action: TcsAction,
}

#[derive(Subcommand, Debug)]
enum TcsAction {
    /// Install a task-specific codemod skill package
    Install(InstallCommand),
    /// Return metadata for a task-specific codemod
    Inspect(InspectCommand),
    /// Run a task-specific codemod explicitly
    Run(RunCommand),
}

#[derive(Args, Debug)]
struct InstallCommand {
    /// TCS package identifier
    #[arg(value_name = "TCS_ID")]
    tcs_id: String,
    /// Target harness adapter
    #[arg(long, value_enum, default_value_t = Harness::Auto)]
    harness: Harness,
    /// Install into current repo workspace
    #[arg(long)]
    project: bool,
    /// Install into user-level skills path
    #[arg(long)]
    user: bool,
    /// Overwrite existing skill files
    #[arg(long)]
    force: bool,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct InspectCommand {
    /// TCS package identifier
    #[arg(value_name = "TCS_ID")]
    tcs_id: String,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct RunCommand {
    /// TCS package identifier
    #[arg(value_name = "TCS_ID")]
    tcs_id: String,
    /// Optional target path for transformation
    #[arg(long)]
    target: Option<PathBuf>,
    /// Run in dry-run mode
    #[arg(long)]
    dry_run: bool,
    /// Parameters passed to TCS runtime in key=value format
    #[arg(long = "param", value_name = "KEY=VALUE")]
    params: Vec<String>,
    /// Existing session identifier
    #[arg(long)]
    session: Option<String>,
    /// Directory used for run artifacts
    #[arg(long)]
    artifacts_dir: Option<PathBuf>,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
}

pub async fn handler(args: &Command) -> Result<()> {
    match &args.action {
        TcsAction::Install(command) => {
            let scope = resolve_install_scope(command.project, command.user)?;
            let adapter = resolve_adapter(command.harness)?;
            let _ = adapter.metadata();
            let request = InstallRequest {
                scope,
                force: command.force,
            };
            let _ = (&command.tcs_id, command.format);
            let _ = adapter.install_skills(&request)?;
            Ok(())
        }
        TcsAction::Inspect(command) => {
            let _ = (&command.tcs_id, command.format);
            bail!("tcs inspect is not implemented yet")
        }
        TcsAction::Run(command) => {
            let _ = (
                &command.tcs_id,
                &command.target,
                command.dry_run,
                &command.params,
                &command.session,
                &command.artifacts_dir,
                command.format,
            );
            bail!("tcs run is not implemented yet")
        }
    }
}
