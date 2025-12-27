use anyhow::Result;
use clap::Args;
use codemod_sandbox::sandbox::{
    engine::{execute_js_with_quickjs, SimpleJsExecutionOptions},
    resolvers::OxcResolver,
};
use codemod_sandbox::utils::project_discovery::find_tsconfig;
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

    let options = SimpleJsExecutionOptions {
        script_path: &absolute_js_file_path,
        resolver,
        console_log_collector: None,
    };

    execute_js_with_quickjs(options).await?;

    Ok(())
}
