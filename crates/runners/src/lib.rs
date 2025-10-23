use std::collections::HashMap;

use async_trait::async_trait;

use butterflow_models::Result;

/// Runner trait for executing commands
#[async_trait]
pub trait Runner: Send + Sync {
    /// Run a command
    async fn run_command(
        &self,
        command: &str,
        env: &HashMap<String, String>,
        admin: bool,
    ) -> Result<String>;
}

pub mod direct_runner;
pub mod docker_runner;
pub mod podman_runner;
