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
}

impl Command {
    pub async fn run(&self) -> Result<()> {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(EnvFilter::from_default_env())
            .with_writer(std::io::stderr)
            .with_ansi(false)
            .try_init();

        tracing::info!("Starting MCP server");

        let service = CodemodMcpServer::new(self.usage_log.clone())
            .serve(transport::stdio())
            .await
            .inspect_err(|e| {
                tracing::error! {"serving error: {:?}", e};
            })?;

        service.waiting().await?;
        Ok(())
    }
}
