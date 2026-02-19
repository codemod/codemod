use crate::commands::harness_adapter::{
    resolve_adapter, resolve_install_scope, Harness, InstallRequest, OutputFormat,
};
use anyhow::{bail, Result};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Args, Debug)]
pub struct Command {
    #[command(subcommand)]
    action: AgentAction,
}

#[derive(Subcommand, Debug)]
enum AgentAction {
    /// Install MCS and baseline codemod skills into harness-specific paths
    InstallSkills(InstallSkillsCommand),
    /// Validate skill metadata, paths, and compatibility markers
    VerifySkills(VerifySkillsCommand),
    /// List installed codemod skills for a harness
    ListSkills(ListSkillsCommand),
    /// Run MCS orchestration for a natural-language migration intent
    Run(RunCommand),
}

#[derive(Args, Debug)]
struct InstallSkillsCommand {
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
struct VerifySkillsCommand {
    /// Target harness adapter
    #[arg(long, value_enum, default_value_t = Harness::Auto)]
    harness: Harness,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct ListSkillsCommand {
    /// Target harness adapter
    #[arg(long, value_enum, default_value_t = Harness::Auto)]
    harness: Harness,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
}

#[derive(Args, Debug)]
struct RunCommand {
    /// Natural language migration intent
    #[arg(value_name = "INTENT")]
    intent: String,
    /// Target harness adapter
    #[arg(long, value_enum, default_value_t = Harness::Auto)]
    harness: Harness,
    /// Existing session identifier
    #[arg(long)]
    session: Option<String>,
    /// Directory used for run artifacts
    #[arg(long)]
    artifacts_dir: Option<PathBuf>,
    /// Max orchestration iterations
    #[arg(long)]
    max_iterations: Option<u32>,
    /// Stop after dry-run planning phase
    #[arg(long)]
    dry_run_only: bool,
    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
    format: OutputFormat,
}

pub async fn handler(args: &Command) -> Result<()> {
    match &args.action {
        AgentAction::InstallSkills(command) => {
            let scope = resolve_install_scope(command.project, command.user)?;
            let adapter = resolve_adapter(command.harness)?;
            let _ = adapter.metadata();
            let request = InstallRequest {
                scope,
                force: command.force,
            };
            let _ = command.format;
            let _ = adapter.install_skills(&request)?;
            Ok(())
        }
        AgentAction::VerifySkills(command) => {
            let adapter = resolve_adapter(command.harness)?;
            let _ = adapter.metadata();
            let _ = command.format;
            let _ = adapter.verify_skills()?;
            Ok(())
        }
        AgentAction::ListSkills(command) => {
            let adapter = resolve_adapter(command.harness)?;
            let _ = adapter.metadata();
            let _ = command.format;
            let _ = adapter.list_skills()?;
            Ok(())
        }
        AgentAction::Run(command) => {
            let _ = (
                &command.intent,
                command.harness,
                &command.session,
                &command.artifacts_dir,
                command.max_iterations,
                command.dry_run_only,
                command.format,
            );
            bail!("agent run is not implemented yet")
        }
    }
}
