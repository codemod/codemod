use anyhow::Result;
use clap::Args;
use codemod_sandbox::sandbox::{
    engine::{execute_js_with_quickjs, SimpleJsExecutionOptions},
    resolvers::OxcResolver,
};
use codemod_sandbox::utils::project_discovery::find_tsconfig;
use codemod_sandbox::SharedStateContext;
use log::warn;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

#[derive(Args, Debug)]
pub struct Command {
    /// Path to the JavaScript file to execute
    pub js_file: String,
}

pub async fn handler(args: &Command) -> Result<()> {
    let js_file_path = Path::new(&args.js_file);

    if !js_file_path.exists() {
        anyhow::bail!(
            "JavaScript file '{}' does not exist",
            js_file_path.display()
        );
    }

    let absolute_js_file_path = js_file_path.canonicalize()?;

    let script_base_dir = absolute_js_file_path
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();

    let tsconfig_path = find_tsconfig(&script_base_dir);

    let resolver = Arc::new(OxcResolver::new(script_base_dir.clone(), tsconfig_path)?);

    // Load workflow state from CODEMOD_STATE env var if running inside a workflow.
    // CODEMOD_STATE points to a temp file containing JSON (avoids env var size limits).
    let shared_state_context = if let Ok(state_file_path) = std::env::var("CODEMOD_STATE") {
        match std::fs::read_to_string(&state_file_path) {
            Ok(state_json) => {
                match serde_json::from_str::<HashMap<String, serde_json::Value>>(&state_json) {
                    Ok(state) => SharedStateContext::with_initial_state(state),
                    Err(e) => {
                        warn!(
                            "Failed to parse CODEMOD_STATE file '{}': {}",
                            state_file_path, e
                        );
                        SharedStateContext::new()
                    }
                }
            }
            Err(e) => {
                warn!(
                    "Failed to read CODEMOD_STATE file '{}': {}",
                    state_file_path, e
                );
                SharedStateContext::new()
            }
        }
    } else {
        SharedStateContext::new()
    };

    let options = SimpleJsExecutionOptions {
        script_path: &absolute_js_file_path,
        resolver,
        metrics_context: None,
        shared_state_context: Some(shared_state_context.clone()),
        console_log_collector: None,
    };

    execute_js_with_quickjs(options).await?;

    // Write state changes to STATE_OUTPUTS file if running inside a workflow
    if let Ok(state_outputs_path) = std::env::var("STATE_OUTPUTS") {
        let persistable = shared_state_context.get_persistable();

        if !persistable.is_empty() {
            let mut lines = Vec::new();
            for (key, value) in &persistable {
                let json_str = serde_json::to_string(value).unwrap_or_default();
                lines.push(format!("{key}={json_str}"));
            }
            std::fs::write(&state_outputs_path, lines.join("\n") + "\n")?;
        }
    }

    Ok(())
}
