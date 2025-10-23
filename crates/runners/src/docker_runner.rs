use std::collections::HashMap;
use std::process::Command;

use async_trait::async_trait;

use butterflow_models::Error;
use butterflow_models::Result;

use crate::Runner;

/// Docker runner (runs commands in Docker containers)
pub struct DockerRunner;

impl DockerRunner {
    /// Create a new Docker runner
    pub fn new() -> Self {
        Self
    }
}

impl Default for DockerRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runner for DockerRunner {
    async fn run_command(
        &self,
        command: &str,
        env: &HashMap<String, String>,
        admin: bool,
    ) -> Result<String> {
        // Create environment variables array
        let env_args: Vec<String> = env
            .iter()
            .map(|(key, value)| format!("--env={key}={value}"))
            .collect();

        // Create a unique container name
        let container_name = format!("butterflow-{}", uuid::Uuid::new_v4());

        // Create the docker command
        let mut cmd = Command::new("docker");
        cmd.arg("run")
            .arg("--rm")
            .arg("--name")
            .arg(&container_name);

        if admin {
            cmd.arg("--privileged");
        }

        cmd.args(env_args)
            .arg("alpine:latest")
            .arg("sh")
            .arg("-c")
            .arg(command);

        // Execute the command
        let output = cmd
            .output()
            .map_err(|e| Error::Docker(format!("Failed to execute docker command: {e}")))?;

        // Check if the command succeeded
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Docker(format!(
                "Docker command failed with exit code {}: {}",
                output.status.code().unwrap_or(-1),
                stderr
            )));
        }

        // Return the output
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(stdout)
    }
}
