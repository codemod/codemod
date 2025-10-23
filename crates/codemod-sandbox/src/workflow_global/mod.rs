use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{prelude::Func, Ctx, Exception, Object, Result};
pub mod types;

#[cfg(feature = "native")]
pub mod native;

#[cfg(feature = "wasm")]
pub mod wasm;

#[allow(dead_code)]
pub(crate) struct WorkflowGlobalModule;

impl ModuleDef for WorkflowGlobalModule {
    fn declare(declare: &Declarations) -> Result<()> {
        declare.declare("setGlobalVariable")?;
        declare.declare("getGlobalVariable")?;
        declare.declare("default")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let default = Object::new(ctx.clone())?;

        #[cfg(feature = "native")]
        {
            default.set("setGlobalVariable", Func::from(set_global_variable_rjs))?;
            default.set("getGlobalVariable", Func::from(get_global_variable_rjs))?;

            exports.export("setGlobalVariable", Func::from(set_global_variable_rjs))?;
            exports.export("getGlobalVariable", Func::from(get_global_variable_rjs))?;
        }

        #[cfg(feature = "wasm")]
        {
            default.set("setGlobalVariable", Func::from(set_global_variable_rjs))?;
            default.set("getGlobalVariable", Func::from(get_global_variable_rjs))?;

            exports.export("setGlobalVariable", Func::from(set_global_variable_rjs))?;
            exports.export("getGlobalVariable", Func::from(get_global_variable_rjs))?;
        }

        exports.export("default", default)?;
        Ok(())
    }
}

#[cfg(feature = "native")]
fn set_global_variable_rjs(ctx: Ctx<'_>, name: String, variable: String) -> Result<()> {
    native::set_global_variable(&name, &variable)
        .map_err(|e| Exception::throw_message(&ctx, &format!("Failed to set global variable: {e}")))
}

#[cfg(feature = "native")]
fn get_global_variable_rjs(ctx: Ctx<'_>, name: String) -> Result<Option<String>> {
    native::get_global_variable(&name)
        .map(|opt| opt.map(|v| v.to_string()))
        .map_err(|e| Exception::throw_message(&ctx, &format!("Failed to get global variable: {e}")))
}

#[cfg(feature = "wasm")]
fn set_global_variable_rjs(ctx: Ctx<'_>, name: String, variable: String) -> Result<()> {
    wasm::set_global_variable(&name, &variable)
        .map_err(|e| Exception::throw_message(&ctx, &format!("Failed to set global variable: {e}")))
}

#[cfg(feature = "wasm")]
fn get_global_variable_rjs(ctx: Ctx<'_>, name: String) -> Result<Option<String>> {
    wasm::get_global_variable(&name)
        .map_err(|e| Exception::throw_message(&ctx, &format!("Failed to get global variable: {e}")))
}

// Helper functions for type-safe access

#[cfg(feature = "wasm")]
fn get_string_variable_rjs(ctx: Ctx<'_>, name: String) -> Result<Option<String>> {
    get_global_variable_rjs(ctx, name)
}

#[cfg(feature = "wasm")]
fn get_number_variable_rjs(ctx: Ctx<'_>, name: String) -> Result<Option<f64>> {
    match wasm::get_global_variable(&name) {
        Ok(Some(value)) => {
            let trimmed = value.trim();
            trimmed.parse::<f64>().map(Some).map_err(|e| {
                Exception::throw_message(&ctx, &format!("Failed to parse as number: {e}"))
            })
        }
        Ok(None) => Ok(None),
        Err(e) => Err(Exception::throw_message(
            &ctx,
            &format!("Failed to get variable: {e}"),
        )),
    }
}

#[cfg(feature = "wasm")]
fn get_json_variable_rjs(ctx: Ctx<'_>, name: String) -> Result<Option<String>> {
    match wasm::get_global_variable(&name) {
        Ok(Some(value)) => {
            let trimmed = value.to_string();
            serde_json::from_str::<serde_json::Value>(trimmed)
                .map(|_| Some(trimmed.to_string()))
                .map_err(|e| {
                    Exception::throw_message(&ctx, &format!("Failed to parse as JSON: {e}"))
                })
        }
        Ok(None) => Ok(None),
        Err(e) => Err(Exception::throw_message(
            &ctx,
            &format!("Failed to get variable: {e}"),
        )),
    }
}
