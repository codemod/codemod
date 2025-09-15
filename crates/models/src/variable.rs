use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;

use crate::error::Error;
use crate::Result;

/// Resolve a dot notation path in a HashMap<String, Value>
/// For example: resolve_state_path(state, "x.y.0.z") gets state["x"]["y"][0]["z"]
pub fn resolve_state_path<'a>(state: &'a HashMap<String, Value>, path: &str) -> Result<&'a Value> {
    let parts: Vec<&str> = path.split('.').collect();

    if parts.is_empty() {
        return Err(Error::VariableResolution("Empty path".to_string()));
    }

    let root_key = parts[0];
    let mut current_value = state
        .get(root_key)
        .ok_or_else(|| Error::VariableResolution(format!("State key not found: {root_key}")))?;

    for part in &parts[1..] {
        current_value = match current_value {
            Value::Object(obj) => obj.get(*part).ok_or_else(|| {
                Error::VariableResolution(format!("Object key not found: {part}"))
            })?,
            Value::Array(arr) => {
                let index: usize = part.parse().map_err(|_| {
                    Error::VariableResolution(format!("Invalid array index: {part}"))
                })?;
                arr.get(index).ok_or_else(|| {
                    Error::VariableResolution(format!("Array index out of bounds: {index}"))
                })?
            }
            _ => {
                return Err(Error::VariableResolution(format!(
                    "Cannot traverse path '{part}' - value is not an object or array"
                )));
            }
        };
    }

    Ok(current_value)
}

/// Resolve variables in a string
pub fn resolve_variables(
    input: &str,
    params: &HashMap<String, String>,
    state: &HashMap<String, Value>,
    matrix_values: Option<&HashMap<String, Value>>,
) -> Result<String> {
    let re = Regex::new(r"\$\{\{([^}]+)\}\}").unwrap();
    let mut result = input.to_string();

    for captures in re.captures_iter(input) {
        let full_match = captures.get(0).unwrap().as_str();
        let inner = captures.get(1).unwrap().as_str().trim();

        let replacement =
            // First check if it's a direct matrix value
            if matrix_values.is_some_and(|matrix_values| matrix_values.contains_key(inner)) {
                serde_json::to_string(matrix_values.unwrap().get(inner).unwrap()).unwrap()
            } else if let Some(name) = inner.strip_prefix("params.") {
                params.get(name).cloned().ok_or_else(|| {
                    Error::VariableResolution(format!("Parameter not found: {name}"))
                })?
            } else if let Some(name) = inner.strip_prefix("state.") {
                let value = resolve_state_path(state, name)?;
                serde_json::to_string(value).unwrap()
            } else {
                return Err(Error::VariableResolution(format!(
                    "Unknown variable type: {inner}"
                )));
            };

        result = result.replace(full_match, &replacement);
    }

    Ok(result)
}
