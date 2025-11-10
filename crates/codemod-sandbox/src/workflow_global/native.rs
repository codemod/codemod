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
        "Error: Failed to {}: STEP_OUTPUTS environment variable not set",
        action
    );
    let full_msg = format!("{}\nCaused by: {}", msg, err);
    Box::new(io::Error::new(io::ErrorKind::NotFound, full_msg))
}

pub fn set_step_output(output_name: &str, value: &str) -> Result<(), Box<dyn std::error::Error>> {
    let step_id = env::var("CODEMOD_STEP_ID").unwrap_or_default();
    let file_path = get_step_outputs_path().map_err(|e| {
        wrap_missing_env_var(e, &format!("set step output '{}.{}'", step_id, output_name))
    })?;

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

    writeln!(file, "{}.{}={}", step_id, output_name, value)?;
    Ok(())
}

pub fn get_step_output(
    step_id: &str,
    output_name: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let file_path = get_step_outputs_path().map_err(|e| {
        wrap_missing_env_var(e, &format!("get step output '{}.{}'", step_id, output_name))
    })?;

    if !file_path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&file_path)?;
    let search_key = format!("{}.{}", step_id, output_name);

    for line in content.lines() {
        if let Some((key, value)) = line.split_once('=') {
            if key.trim() == search_key {
                return Ok(Some(value.to_string()));
            }
        }
    }

    Ok(None)
}

pub fn get_step_outputs(
    step_id: &str,
) -> Result<std::collections::HashMap<String, String>, Box<dyn std::error::Error>> {
    let file_path = get_step_outputs_path()
        .map_err(|e| wrap_missing_env_var(e, &format!("get step outputs for '{}'", step_id)))?;

    let mut outputs = std::collections::HashMap::new();

    if !file_path.exists() {
        return Ok(outputs);
    }

    let content = fs::read_to_string(&file_path)?;
    let prefix = format!("{}.", step_id);

    for line in content.lines() {
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            if key.starts_with(&prefix) {
                if let Some(output_name) = key.strip_prefix(&prefix) {
                    outputs.insert(output_name.to_string(), value.to_string());
                }
            }
        }
    }

    Ok(outputs)
}

fn get_step_outputs_path() -> Result<PathBuf, io::Error> {
    env::var("STEP_OUTPUTS").map(PathBuf::from).map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "STEP_OUTPUTS environment variable not set",
        )
    })
}
