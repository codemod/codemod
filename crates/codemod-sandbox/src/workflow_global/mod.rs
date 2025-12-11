use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{prelude::Func, Ctx, Exception, Object, Result};
use std::collections::HashMap;
use std::env;
use std::sync::{Arc, Condvar, LazyLock, Mutex};

static STEP_OUTPUTS_STORE: LazyLock<Mutex<HashMap<String, HashMap<String, String>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

static STEP_OUTPUT_NOTIFIER: LazyLock<Arc<Condvar>> = LazyLock::new(|| Arc::new(Condvar::new()));

pub fn set_step_output(
    output_name: &str,
    value: &str,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let step_id = env::var("CODEMOD_STEP_ID").unwrap_or_default();

    {
        let mut store = STEP_OUTPUTS_STORE.lock().unwrap();
        store
            .entry(step_id)
            .or_default()
            .insert(output_name.to_string(), value.to_string());
    }

    STEP_OUTPUT_NOTIFIER.notify_all();

    Ok(())
}

pub fn get_step_output(
    step_id: &str,
    output_name: &str,
) -> std::result::Result<Option<String>, Box<dyn std::error::Error>> {
    let store = STEP_OUTPUTS_STORE.lock().unwrap();

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
    let store = STEP_OUTPUTS_STORE.lock().unwrap();

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
    let mut store = STEP_OUTPUTS_STORE.lock().unwrap();

    let outputs = store.entry(step_id.to_string()).or_default();

    if let Some(value) = outputs.get(output_name) {
        // Output already exists, return it
        Ok(value.clone())
    } else {
        // Output doesn't exist, set it and return the new value
        outputs.insert(output_name.to_string(), default_value.to_string());
        drop(store); // Release lock before notifying
        STEP_OUTPUT_NOTIFIER.notify_all();
        Ok(default_value.to_string())
    }
}

#[allow(dead_code)]
pub(crate) struct WorkflowGlobalModule;

impl ModuleDef for WorkflowGlobalModule {
    fn declare(declare: &Declarations) -> Result<()> {
        declare.declare("setStepOutput")?;
        declare.declare("getStepOutput")?;
        declare.declare("waitForStepOutput")?;
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
