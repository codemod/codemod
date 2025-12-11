use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{prelude::Func, Ctx, Exception, Object, Result};
use std::collections::HashMap;
use std::env;
use std::sync::{LazyLock, Mutex};

static STEP_OUTPUTS_STORE: LazyLock<Mutex<HashMap<String, HashMap<String, String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn set_step_output(
    output_name: &str,
    value: &str,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let step_id = env::var("CODEMOD_STEP_ID").unwrap_or_default();

    {
        let mut store = STEP_OUTPUTS_STORE
            .lock()
            .map_err(|e| format!("Failed to lock STEP_OUTPUTS_STORE: {}", e))?;
        store
            .entry(step_id)
            .or_default()
            .insert(output_name.to_string(), value.to_string());
    }

    Ok(())
}

pub fn get_step_output(
    step_id: &str,
    output_name: &str,
) -> std::result::Result<Option<String>, Box<dyn std::error::Error>> {
    let store = STEP_OUTPUTS_STORE
        .lock()
        .map_err(|e| format!("Failed to lock STEP_OUTPUTS_STORE: {}", e))?;

    if let Some(outputs) = store.get(step_id) {
        if let Some(value) = outputs.get(output_name) {
            return Ok(Some(value.clone()));
        }
    }

    Ok(None)
}

pub fn get_step_outputs(
    step_id: &str,
) -> std::result::Result<HashMap<String, String>, Box<dyn std::error::Error>> {
    let store = STEP_OUTPUTS_STORE
        .lock()
        .map_err(|e| format!("Failed to lock STEP_OUTPUTS_STORE: {}", e))?;

    if let Some(outputs) = store.get(step_id) {
        Ok(outputs.clone())
    } else {
        Ok(HashMap::new())
    }
}

/// Get or set step output atomically
/// If the output exists, returns it. If not, sets it to the provided value and returns it.
pub fn get_or_set_step_output(
    step_id: &str,
    output_name: &str,
    default_value: &str,
) -> std::result::Result<String, Box<dyn std::error::Error>> {
    let mut store = STEP_OUTPUTS_STORE
        .lock()
        .map_err(|e| format!("Failed to lock STEP_OUTPUTS_STORE: {}", e))?;

    let outputs = store.entry(step_id.to_string()).or_default();

    if let Some(value) = outputs.get(output_name) {
        // Output already exists, return it
        Ok(value.clone())
    } else {
        // Output doesn't exist, set it and return the new value
        outputs.insert(output_name.to_string(), default_value.to_string());
        drop(store); // Release lock before notifying
        Ok(default_value.to_string())
    }
}

#[allow(dead_code)]
pub(crate) struct WorkflowGlobalModule;

impl ModuleDef for WorkflowGlobalModule {
    fn declare(declare: &Declarations) -> Result<()> {
        declare.declare("setStepOutput")?;
        declare.declare("getStepOutput")?;
        declare.declare("getOrSetStepOutput")?;
        declare.declare("default")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let default = Object::new(ctx.clone())?;

        default.set("setStepOutput", Func::from(set_step_output_rjs))?;
        default.set("getStepOutput", Func::from(get_step_output_rjs))?;
        default.set("getOrSetStepOutput", Func::from(get_or_set_step_output_rjs))?;

        exports.export("setStepOutput", Func::from(set_step_output_rjs))?;
        exports.export("getStepOutput", Func::from(get_step_output_rjs))?;
        exports.export("getOrSetStepOutput", Func::from(get_or_set_step_output_rjs))?;

        exports.export("default", default)?;
        Ok(())
    }
}

fn set_step_output_rjs(ctx: Ctx<'_>, output_name: String, value: String) -> Result<()> {
    let result = set_step_output(&output_name, &value);
    result.map_err(|e| Exception::throw_message(&ctx, &format!("Failed to set step output: {e}")))
}

fn get_step_output_rjs(
    ctx: Ctx<'_>,
    step_id: String,
    output_name: String,
) -> Result<Option<String>> {
    let result = get_step_output(&step_id, &output_name);
    result.map_err(|e| Exception::throw_message(&ctx, &format!("Failed to get step output: {e}")))
}

fn get_or_set_step_output_rjs(
    ctx: Ctx<'_>,
    step_id: String,
    output_name: String,
    default_value: String,
) -> Result<String> {
    let result = get_or_set_step_output(&step_id, &output_name, &default_value);
    result.map_err(|e| {
        Exception::throw_message(&ctx, &format!("Failed to get or set step output: {e}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn test_get_or_set_step_output_sets_when_empty() {
        let step_id = "test_step_1";
        let output_name = "test_output";
        let default_value = "default_value";

        let result = get_or_set_step_output(step_id, output_name, default_value).unwrap();
        assert_eq!(result, default_value);

        // Verify it was actually set
        let stored = get_step_output(step_id, output_name).unwrap();
        assert_eq!(stored, Some(default_value.to_string()));
    }

    #[test]
    fn test_get_or_set_step_output_gets_existing() {
        let step_id = "test_step_2";
        let output_name = "test_output";
        let initial_value = "initial_value";
        let default_value = "default_value";

        // Temporarily set the step ID for the test
        unsafe {
            std::env::set_var("CODEMOD_STEP_ID", step_id);
        }
        set_step_output(output_name, initial_value).unwrap();
        unsafe {
            std::env::remove_var("CODEMOD_STEP_ID");
        }

        // Try to get or set with different default
        let result = get_or_set_step_output(step_id, output_name, default_value).unwrap();

        // Should return the existing value, not the default
        assert_eq!(result, initial_value);
    }

    #[test]
    fn test_get_or_set_step_output_different_steps() {
        let step_id_1 = "test_step_3a";
        let step_id_2 = "test_step_3b";
        let output_name = "test_output";
        let value_1 = "value_1";
        let value_2 = "value_2";

        let result_1 = get_or_set_step_output(step_id_1, output_name, value_1).unwrap();
        let result_2 = get_or_set_step_output(step_id_2, output_name, value_2).unwrap();

        assert_eq!(result_1, value_1);
        assert_eq!(result_2, value_2);

        // Verify both are stored independently
        assert_eq!(
            get_step_output(step_id_1, output_name).unwrap(),
            Some(value_1.to_string())
        );
        assert_eq!(
            get_step_output(step_id_2, output_name).unwrap(),
            Some(value_2.to_string())
        );
    }

    #[test]
    fn test_get_or_set_step_output_concurrent_access() {
        let step_id = "test_step_4";
        let output_name = "test_output";
        let num_threads = 10;

        let handles: Vec<_> = (0..num_threads)
            .map(|i| {
                let step_id = step_id.to_string();
                let output_name = output_name.to_string();
                let default_value = format!("value_{}", i);

                thread::spawn(move || {
                    get_or_set_step_output(&step_id, &output_name, &default_value).unwrap()
                })
            })
            .collect();

        let results: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // All threads should get the same value (the one set by the first thread to acquire the lock)
        let first_value = &results[0];
        for result in &results {
            assert_eq!(result, first_value);
        }

        // Verify the final stored value matches what all threads got
        let stored = get_step_output(step_id, output_name).unwrap();
        assert_eq!(stored, Some(first_value.clone()));
    }

    #[test]
    fn test_get_or_set_step_output_multiple_outputs_per_step() {
        let step_id = "test_step_5";
        let output_1 = "output_1";
        let output_2 = "output_2";
        let value_1 = "value_1";
        let value_2 = "value_2";

        let result_1 = get_or_set_step_output(step_id, output_1, value_1).unwrap();
        let result_2 = get_or_set_step_output(step_id, output_2, value_2).unwrap();

        assert_eq!(result_1, value_1);
        assert_eq!(result_2, value_2);

        // Verify both outputs are stored
        let all_outputs = get_step_outputs(step_id).unwrap();
        assert_eq!(all_outputs.len(), 2);
        assert_eq!(all_outputs.get(output_1), Some(&value_1.to_string()));
        assert_eq!(all_outputs.get(output_2), Some(&value_2.to_string()));
    }

    #[test]
    fn test_set_and_get_step_output_basic() {
        let output_name = "test_output_basic";
        let value = "test_value";

        unsafe {
            std::env::set_var("CODEMOD_STEP_ID", "test_step_6");
        }

        set_step_output(output_name, value).unwrap();
        let result = get_step_output("test_step_6", output_name).unwrap();

        unsafe {
            std::env::remove_var("CODEMOD_STEP_ID");
        }

        assert_eq!(result, Some(value.to_string()));
    }

    #[test]
    fn test_get_step_output_nonexistent() {
        let result = get_step_output("nonexistent_step", "nonexistent_output").unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_get_step_outputs_empty() {
        let result = get_step_outputs("empty_step").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_concurrent_different_outputs() {
        let step_id = "test_step_7";
        let num_threads = 5;

        let handles: Vec<_> = (0..num_threads)
            .map(|i| {
                let step_id = step_id.to_string();
                let output_name = format!("output_{}", i);
                let value = format!("value_{}", i);

                thread::spawn(move || {
                    get_or_set_step_output(&step_id, &output_name, &value).unwrap()
                })
            })
            .collect();

        let results: Vec<String> = handles.into_iter().map(|h| h.join().unwrap()).collect();

        // Each thread should have successfully set its own output
        for (i, result) in results.iter().enumerate() {
            assert_eq!(result, &format!("value_{}", i));
        }

        // Verify all outputs are stored
        let all_outputs = get_step_outputs(step_id).unwrap();
        assert_eq!(all_outputs.len(), num_threads);
    }

    #[test]
    fn test_get_or_set_with_complex_json_value() {
        let step_id = "test_step_8";
        let output_name = "json_output";
        let json_value = r#"{"key": "value", "nested": {"array": [1, 2, 3]}}"#;

        let result = get_or_set_step_output(step_id, output_name, json_value).unwrap();
        assert_eq!(result, json_value);

        let stored = get_step_output(step_id, output_name).unwrap();
        assert_eq!(stored, Some(json_value.to_string()));
    }
}
