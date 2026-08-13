use anyhow::Result;
use clap::{Args, Subcommand};

pub mod set_repository;

#[derive(Args, Debug)]
pub struct Command {
    #[command(subcommand)]
    action: ManifestAction,
}

#[derive(Subcommand, Debug)]
enum ManifestAction {
    /// Set the source repository metadata in codemod.yaml
    SetRepository(set_repository::Command),
}

pub fn handler(args: &Command) -> Result<()> {
    match &args.action {
        ManifestAction::SetRepository(args) => set_repository::handler(args),
    }
}
