use crate::feedback;
use anyhow::Result;
use clap::Args;
use codemod_mcp::CodemodMcpServer;
use rmcp::{transport, ServiceExt};
use std::path::PathBuf;
use tracing_subscriber::{self, EnvFilter};

#[derive(Args, Debug)]
pub struct Command {
    /// Write MCP usage events to a file for debugging
    #[arg(long)]
    usage_log: Option<PathBuf>,
    /// Opt in to anonymous Codemod AI and MCP feedback for this OS user
    #[arg(long)]
    allow_feedback: bool,
}

impl Command {
    pub async fn run(&self) -> Result<()> {
        feedback::persist_feedback_consent_if_requested(self.allow_feedback)?;

        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .try_init();

        tracing::info!("Starting MCP server");

        let service = CodemodMcpServer::new_with_feedback(
            self.usage_log.clone(),
            feedback::anonymous_feedback_client("mcp")?,
        )
        .serve(transport::stdio())
        .await
        .inspect_err(|e| {
            tracing::error! {"serving error: {:?}", e};
        })?;

        service.waiting().await?;
        Ok(())
    }
}
