use std::env;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;

#[allow(unused_imports)]
use super::types::WorkflowGlobalError;

fn wrap_missing_env_var<E: std::error::Error + 'static>(
    err: E,
    action: &str,
) -> Box<dyn std::error::Error> {
    let msg = format!(
        "Error: Failed to {}: WORKFLOW_GLOBAL environment variable not set",
        action
    );
    let full_msg = format!("{}\nCaused by: {}", msg, err);
    Box::new(io::Error::new(io::ErrorKind::NotFound, full_msg))
}

pub fn set_global_variable(name: &str, variable: &str) -> Result<(), Box<dyn std::error::Error>> {
    let file_path = get_workflow_global_path()
        .map_err(|e| wrap_missing_env_var(e, &format!("set global variable '{}'", name)))?;

    if !file_path.exists() {
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::File::create(&file_path)?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file_path)?;

    writeln!(file, "{}={}", name, variable)?;
    Ok(())
}

pub fn get_global_variable(name: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let file_path = get_workflow_global_path()
        .map_err(|e| wrap_missing_env_var(e, &format!("get global variable '{}'", name)))?;

    if !file_path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&file_path)?;

    for line in content.lines() {
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == name {
                return Ok(Some(value.to_string()));
            }
        }
    }

    Ok(None)
}

fn get_workflow_global_path() -> Result<PathBuf, io::Error> {
    env::var("WORKFLOW_GLOBAL").map(PathBuf::from).map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "WORKFLOW_GLOBAL environment variable not set",
        )
    })
}
