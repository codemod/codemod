#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
mod wasm_capabilities {

    use std::future::Future;

    use rquickjs::{
        module::{Declarations, Exports, ModuleDef},
        prelude::Async,
        Ctx, Function, Result, Value,
    };
    use wasm_bindgen::prelude::*;

    pub struct CapabilitiesModule;

    impl ModuleDef for CapabilitiesModule {
        fn declare(decl: &Declarations) -> Result<()> {
            decl.declare("fetch")?;
            Ok(())
        }

        fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
            let context = ctx.clone();
            let fetch_function =
                Function::new(context.clone(), Async(execute_fetch))?.with_name("fetch")?;
            exports.export("fetch", fetch_function)?;
            Ok(())
        }
    }

    async fn execute_capability_with_serialization<'js, CapabilityFn, FutureResult>(
        request_id: String,
        input_value: Value<'js>,
        capability_handler: CapabilityFn,
    ) -> Result<Value<'js>>
    where
        CapabilityFn: FnOnce(String, String) -> FutureResult,
        FutureResult: Future<Output = JsValue>,
    {
        let js_context: Ctx<'js> = input_value.ctx().clone();

        // Serialize input to JSON string
        let serialized_input = js_context.json_stringify(input_value)?.unwrap();
        let input_string = serialized_input.to_string()?;

        // Execute the capability and get result
        let capability_result = capability_handler(request_id, input_string).await;
        let result_json = capability_result.as_string().unwrap_or_default();

        // Process any pending JavaScript jobs
        while js_context.execute_pending_job() {}

        // Parse the result back to a JavaScript value
        js_context.json_parse(result_json)
    }

    macro_rules! generate_capability_wrapper {
        ($wrapper_name:ident, $external_capability:ident) => {
            async fn $wrapper_name<'js>(
                request_id: String,
                input_data: Value<'js>,
            ) -> Result<Value<'js>> {
                execute_capability_with_serialization(request_id, input_data, $external_capability)
                    .await
            }
        };
    }

    // Create the capability wrappers
    generate_capability_wrapper!(execute_fetch, fetch);

    #[wasm_bindgen(raw_module = "./capabilities.js")]
    extern "C" {

        async fn fetch(invocation_id: String, inputs: String) -> JsValue;
    }
}

#[cfg(all(feature = "wasm", target_arch = "wasm32"))]
pub use wasm_capabilities::*;
