use std::collections::HashMap;
use std::sync::RwLock;

#[allow(unused_imports)]
use super::types::WorkflowGlobalError;

static STEP_OUTPUTS_STORE: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

fn ensure_store_initialized() {
    let mut store = STEP_OUTPUTS_STORE.write().unwrap();
    if store.is_none() {
        *store = Some(HashMap::new());
    }
}

pub fn set_step_output(output_name: &str, value: &str) -> Result<(), Box<dyn std::error::Error>> {
    let step_id = env::var("CODEMOD_STEP_ID").unwrap_or_default();
    ensure_store_initialized();
    let mut store = STEP_OUTPUTS_STORE.write().unwrap();
    if let Some(map) = store.as_mut() {
        let key = format!("{}.{}", step_id, output_name);
        map.insert(key, value.to_string());
    }

    Ok(())
}

pub fn get_step_output(
    step_id: &str,
    output_name: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    ensure_store_initialized();

    let store = STEP_OUTPUTS_STORE.read().unwrap();
    if let Some(map) = store.as_ref() {
        let key = format!("{}.{}", step_id, output_name);
        if let Some(value) = map.get(&key) {
            return Ok(Some(value.clone()));
        }
    }

    Ok(None)
}

pub fn get_step_outputs(
    step_id: &str,
) -> Result<HashMap<String, String>, Box<dyn std::error::Error>> {
    ensure_store_initialized();

    let mut outputs = HashMap::new();
    let store = STEP_OUTPUTS_STORE.read().unwrap();

    if let Some(map) = store.as_ref() {
        let prefix = format!("{}.", step_id);
        for (key, value) in map.iter() {
            if key.starts_with(&prefix) {
                if let Some(output_name) = key.strip_prefix(&prefix) {
                    outputs.insert(output_name.to_string(), value.clone());
                }
            }
        }
    }

    Ok(outputs)
}

#[deprecated(note = "Use set_step_output instead")]
pub fn set_global_variable(name: &str, variable: &str) -> Result<(), Box<dyn std::error::Error>> {
    set_step_output(name, variable)
}

#[deprecated(note = "Use get_step_output instead")]
pub fn get_global_variable(name: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    get_step_output("global", name)
}
