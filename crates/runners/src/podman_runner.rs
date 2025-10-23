use std::collections::HashMap;
use std::process::Command;

use async_trait::async_trait;

use butterflow_models::Error;
use butterflow_models::Result;

use crate::Runner;

/// Podman runner (runs commands in Podman containers)
pub struct PodmanRunner;

impl PodmanRunner {
    /// Create a new Podman runner
    pub fn new() -> Self {
        Self
    }
}

impl Default for PodmanRunner {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Runner for PodmanRunner {
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

        // Create the podman command
        let mut cmd = Command::new("podman");
        cmd.arg("run").arg("--rm");

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
            .map_err(|e| Error::Runtime(format!("Failed to execute podman command: {e}")))?;

        // Check if the command succeeded
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Runtime(format!(
                "Podman command failed with exit code {}: {}",
                output.status.code().unwrap_or(-1),
                stderr
            )));
        }

        // Return the output
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        Ok(stdout)
    }
}

// No need to re-export the runners since they're already public
