use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use butterflow_models::Result;

pub type OutputCallback = Arc<dyn Fn(String) + Send + Sync>;

/// Runner trait for executing commands
#[async_trait]
pub trait Runner: Send + Sync {
    /// Run a command
    async fn run_command(
        &self,
        command: &str,
        env: &HashMap<String, String>,
        output_callback: Option<OutputCallback>,
    ) -> Result<String>;
}

pub mod direct_runner;
pub mod docker_runner;
pub mod podman_runner;
