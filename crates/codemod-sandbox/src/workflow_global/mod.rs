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
        declare.declare("setStepOutput")?;
        declare.declare("getStepOutput")?;
        declare.declare("default")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let default = Object::new(ctx.clone())?;

        default.set("setStepOutput", Func::from(set_step_output_rjs))?;
        default.set("getStepOutput", Func::from(get_step_output_rjs))?;

        exports.export("setStepOutput", Func::from(set_step_output_rjs))?;
        exports.export("getStepOutput", Func::from(get_step_output_rjs))?;

        exports.export("default", default)?;
        Ok(())
    }
}

fn set_step_output_rjs(ctx: Ctx<'_>, output_name: String, value: String) -> Result<()> {
    #[cfg(feature = "native")]
    let result = native::set_step_output(&output_name, &value);
    #[cfg(feature = "wasm")]
    let result = wasm::set_step_output(&output_name, &value);

    result.map_err(|e| Exception::throw_message(&ctx, &format!("Failed to set step output: {e}")))
}

fn get_step_output_rjs(
    ctx: Ctx<'_>,
    step_id: String,
    output_name: String,
) -> Result<Option<String>> {
    #[cfg(feature = "native")]
    let result = native::get_step_output(&step_id, &output_name);
    #[cfg(feature = "wasm")]
    let result = wasm::get_step_output(&step_id, &output_name);

    result.map_err(|e| Exception::throw_message(&ctx, &format!("Failed to get step output: {e}")))
}
