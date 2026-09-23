use crate::utils::quickjs_wasm::quickjs_value_to_jsvalue;
use rquickjs::{Ctx, Object, Result, Value, function::Func, prelude::Rest};
use wasm_bindgen::{JsValue, prelude::*};

pub fn init(ctx: &Ctx<'_>) -> Result<()> {
    let globals = ctx.globals();

    let console = Object::new(ctx.clone())?;

    console.set("log", Func::from(console_log))?;
    console.set("error", Func::from(console_error))?;
    console.set("warn", Func::from(console_warn))?;

    globals.set("console", console)?;

    Ok(())
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = console, js_name = log, variadic)]
    fn host_log(items: Vec<JsValue>);
    #[wasm_bindgen(js_namespace = console, js_name = warn, variadic)]
    fn host_warn(items: Vec<JsValue>);
    #[wasm_bindgen(js_namespace = console, js_name = error, variadic)]
    fn host_error(items: Vec<JsValue>);
}

fn console_log(items: Rest<Value>) {
    let items = convert_from_qj_to_host(items);
    host_log(items);
}

fn console_warn(items: Rest<Value>) {
    let items = convert_from_qj_to_host(items);
    host_warn(items);
}

fn console_error(items: Rest<Value>) {
    let items = convert_from_qj_to_host(items);
    host_error(items);
}

fn convert_from_qj_to_host(items: Rest<Value>) -> Vec<JsValue> {
    let items: Vec<_> = items
        .0
        .into_iter()
        .map(|item| {
            quickjs_value_to_jsvalue(item)
                .unwrap_or_else(|_| JsValue::from_str("<Unrepresentable value>"))
        })
        .collect();
    items
}
