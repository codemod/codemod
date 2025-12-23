use rquickjs::{Ctx, Result, String as RquickjsString};
use wasm_bindgen::prelude::*;

pub fn init(ctx: &Ctx<'_>) -> Result<()> {
    use rquickjs::function::Func;
    let globals = ctx.globals();

    globals.set("atob", Func::from(qj_atob))?;
    globals.set("btoa", Func::from(qj_btoa))?;

    Ok(())
}

#[wasm_bindgen]
extern "C" {
    fn atob(s: &str) -> String;
    fn btoa(s: &str) -> String;
}

fn qj_atob(s: RquickjsString) -> Result<RquickjsString> {
    let ctx = s.ctx().clone();
    let s = s.to_string().unwrap();
    let result = atob(s.as_str());
    RquickjsString::from_str(ctx, result.as_str())
}

fn qj_btoa(s: RquickjsString) -> Result<RquickjsString> {
    let ctx = s.ctx().clone();
    let s = s.to_string().unwrap();
    let result = btoa(s.as_str());
    RquickjsString::from_str(ctx, result.as_str())
}
