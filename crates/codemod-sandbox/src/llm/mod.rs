use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{Ctx, Exception, Function, JsLifetime, Result, Value, prelude::Async};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LlmRequest {
    pub prompt: String,
    pub system_prompt: Option<String>,
    pub output_schema: Option<JsonValue>,
    pub max_tokens: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmResponse {
    pub output: String,
}

pub type LlmRequestFuture =
    Pin<Box<dyn Future<Output = std::result::Result<LlmResponse, String>> + Send>>;
pub type LlmRequestHandler = Arc<dyn Fn(LlmRequest) -> LlmRequestFuture + Send + Sync>;

#[derive(Clone, Default)]
pub struct LlmRuntimeContext {
    handler: Option<LlmRequestHandler>,
}

unsafe impl<'js> JsLifetime<'js> for LlmRuntimeContext {
    type Changed<'to> = LlmRuntimeContext;
}

impl LlmRuntimeContext {
    pub fn new(handler: Option<LlmRequestHandler>) -> Self {
        Self { handler }
    }

    async fn generate(&self, request: LlmRequest) -> std::result::Result<LlmResponse, String> {
        let handler = self
            .handler
            .as_ref()
            .ok_or_else(|| "Engine LLM client is not configured".to_string())?;
        handler(request).await
    }
}

pub struct LlmModule;

impl ModuleDef for LlmModule {
    fn declare(declare: &Declarations) -> Result<()> {
        declare.declare("generate")?;
        declare.declare("default")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let generate = Function::new(ctx.clone(), Async(generate))?;
        exports.export("generate", generate.clone())?;
        exports.export("default", generate)?;
        Ok(())
    }
}

async fn generate<'js>(ctx: Ctx<'js>, request: Value<'js>) -> Result<Value<'js>> {
    let serialized = ctx
        .json_stringify(request)?
        .ok_or_else(|| Exception::throw_message(&ctx, "LLM request cannot be undefined"))?
        .to_string()?;
    let request: LlmRequest = serde_json::from_str(&serialized).map_err(|error| {
        Exception::throw_message(&ctx, &format!("Invalid LLM request: {error}"))
    })?;
    if request.prompt.trim().is_empty() {
        return Err(Exception::throw_message(
            &ctx,
            "LLM request prompt must not be empty",
        ));
    }

    let runtime = ctx
        .userdata::<LlmRuntimeContext>()
        .ok_or_else(|| Exception::throw_message(&ctx, "LlmRuntimeContext not found in userdata"))?
        .clone();
    let response = runtime
        .generate(request)
        .await
        .map_err(|error| Exception::throw_message(&ctx, &error))?;
    let serialized = serde_json::to_string(&response)
        .map_err(|error| Exception::throw_message(&ctx, &error.to_string()))?;
    ctx.json_parse(serialized)
}
