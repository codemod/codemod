use std::collections::HashMap;
use std::sync::RwLock;

#[allow(unused_imports)]
use super::types::WorkflowGlobalError;

static GLOBAL_STORE: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);

fn ensure_store_initialized() {
    let mut store = GLOBAL_STORE.write().unwrap();
    if store.is_none() {
        *store = Some(HashMap::new());
    }
}

pub fn set_global_variable(name: &str, variable: &str) -> Result<(), Box<dyn std::error::Error>> {
    ensure_store_initialized();

    let mut store = GLOBAL_STORE.write().unwrap();
    if let Some(map) = store.as_mut() {
        map.insert(name.to_string(), variable.to_string());
    }

    Ok(())
}

pub fn get_global_variable(name: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    ensure_store_initialized();

    let store = GLOBAL_STORE.read().unwrap();
    if let Some(map) = store.as_ref() {
        if let Some(value) = map.get(name) {
            return Ok(Some(value.clone()));
        }
    }

    Ok(None)
}
