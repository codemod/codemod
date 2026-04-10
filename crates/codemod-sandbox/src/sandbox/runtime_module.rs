use rquickjs::module::{Declarations, Exports, ModuleDef};
use rquickjs::{prelude::Opt, Ctx, Exception, JsLifetime, Object, Result, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use thiserror::Error;

pub type RuntimeEventCallback = Arc<dyn Fn(RuntimeEvent) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeEventKind {
    Progress,
    Warn,
    SetCurrentUnit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeEvent {
    pub kind: RuntimeEventKind,
    pub message: String,
    pub meta: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RuntimeFailureKind {
    #[error("file")]
    File,
    #[error("step")]
    Step,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{kind} failure: {message}")]
pub struct RuntimeFailure {
    pub kind: RuntimeFailureKind,
    pub message: String,
    pub meta: Option<String>,
}

#[derive(Clone, Default)]
pub struct RuntimeHooksContext {
    pub event_callback: Option<RuntimeEventCallback>,
    pub cancellation_flag: Option<Arc<AtomicBool>>,
    pending_failure: Arc<Mutex<Option<RuntimeFailure>>>,
}

unsafe impl<'js> JsLifetime<'js> for RuntimeHooksContext {
    type Changed<'to> = RuntimeHooksContext;
}

impl RuntimeHooksContext {
    pub fn new(
        event_callback: Option<RuntimeEventCallback>,
        cancellation_flag: Option<Arc<AtomicBool>>,
    ) -> Self {
        Self {
            event_callback,
            cancellation_flag,
            pending_failure: Arc::new(Mutex::new(None)),
        }
    }

    pub fn emit(&self, event: RuntimeEvent) {
        if let Some(callback) = &self.event_callback {
            callback(event);
        }
    }

    pub fn is_canceled(&self) -> bool {
        self.cancellation_flag
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    pub fn set_pending_failure(&self, failure: RuntimeFailure) {
        if let Ok(mut pending_failure) = self.pending_failure.lock() {
            *pending_failure = Some(failure);
        }
    }

    pub fn take_pending_failure(&self) -> Option<RuntimeFailure> {
        self.pending_failure
            .lock()
            .ok()
            .and_then(|mut pending_failure| pending_failure.take())
    }
}

pub struct RuntimeModule;

impl ModuleDef for RuntimeModule {
    fn declare(declare: &Declarations) -> Result<()> {
        declare.declare("progress")?;
        declare.declare("warn")?;
        declare.declare("setCurrentUnit")?;
        declare.declare("failFile")?;
        declare.declare("failStep")?;
        declare.declare("isCanceled")?;
        declare.declare("default")?;
        Ok(())
    }

    fn evaluate<'js>(ctx: &Ctx<'js>, exports: &Exports<'js>) -> Result<()> {
        let default = Object::new(ctx.clone())?;

        let progress = rquickjs::Function::new(ctx.clone(), progress)?;
        let warn = rquickjs::Function::new(ctx.clone(), warn)?;
        let set_current_unit = rquickjs::Function::new(ctx.clone(), set_current_unit)?;
        let fail_file = rquickjs::Function::new(ctx.clone(), fail_file)?;
        let fail_step = rquickjs::Function::new(ctx.clone(), fail_step)?;
        let is_canceled = rquickjs::Function::new(ctx.clone(), is_canceled)?;

        default.set("progress", progress.clone())?;
        default.set("warn", warn.clone())?;
        default.set("setCurrentUnit", set_current_unit.clone())?;
        default.set("failFile", fail_file.clone())?;
        default.set("failStep", fail_step.clone())?;
        default.set("isCanceled", is_canceled.clone())?;

        exports.export("progress", progress)?;
        exports.export("warn", warn)?;
        exports.export("setCurrentUnit", set_current_unit)?;
        exports.export("failFile", fail_file)?;
        exports.export("failStep", fail_step)?;
        exports.export("isCanceled", is_canceled)?;
        exports.export("default", default)?;
        Ok(())
    }
}

fn runtime_hooks_context<'js>(ctx: &Ctx<'js>) -> Result<RuntimeHooksContext> {
    ctx.userdata::<RuntimeHooksContext>()
        .ok_or_else(|| Exception::throw_message(ctx, "RuntimeHooksContext not found in userdata"))
        .map(|runtime_hooks_context| runtime_hooks_context.clone())
}

fn serialize_meta<'js>(ctx: &Ctx<'js>, meta: Opt<Value<'js>>) -> Result<Option<String>> {
    match meta.0 {
        Some(value) if !value.is_null() && !value.is_undefined() => Ok(ctx
            .json_stringify(value)?
            .map(|serialized| serialized.to_string())
            .transpose()?),
        _ => Ok(None),
    }
}

fn progress<'js>(ctx: Ctx<'js>, message: String, meta: Opt<Value<'js>>) -> Result<()> {
    let runtime_hooks_context = runtime_hooks_context(&ctx)?;
    runtime_hooks_context.emit(RuntimeEvent {
        kind: RuntimeEventKind::Progress,
        message,
        meta: serialize_meta(&ctx, meta)?,
    });
    Ok(())
}

fn warn<'js>(ctx: Ctx<'js>, message: String, meta: Opt<Value<'js>>) -> Result<()> {
    let runtime_hooks_context = runtime_hooks_context(&ctx)?;
    runtime_hooks_context.emit(RuntimeEvent {
        kind: RuntimeEventKind::Warn,
        message,
        meta: serialize_meta(&ctx, meta)?,
    });
    Ok(())
}

fn set_current_unit<'js>(ctx: Ctx<'js>, unit_id: String, meta: Opt<Value<'js>>) -> Result<()> {
    let runtime_hooks_context = runtime_hooks_context(&ctx)?;
    runtime_hooks_context.emit(RuntimeEvent {
        kind: RuntimeEventKind::SetCurrentUnit,
        message: unit_id,
        meta: serialize_meta(&ctx, meta)?,
    });
    Ok(())
}

fn fail_file<'js>(ctx: Ctx<'js>, message: String, meta: Opt<Value<'js>>) -> Result<()> {
    let runtime_hooks_context = runtime_hooks_context(&ctx)?;
    runtime_hooks_context.set_pending_failure(RuntimeFailure {
        kind: RuntimeFailureKind::File,
        message: message.clone(),
        meta: serialize_meta(&ctx, meta)?,
    });
    Err(Exception::throw_message(&ctx, &message))
}

fn fail_step<'js>(ctx: Ctx<'js>, message: String, meta: Opt<Value<'js>>) -> Result<()> {
    let runtime_hooks_context = runtime_hooks_context(&ctx)?;
    runtime_hooks_context.set_pending_failure(RuntimeFailure {
        kind: RuntimeFailureKind::Step,
        message: message.clone(),
        meta: serialize_meta(&ctx, meta)?,
    });
    Err(Exception::throw_message(&ctx, &message))
}

fn is_canceled<'js>(ctx: Ctx<'js>) -> Result<bool> {
    Ok(runtime_hooks_context(&ctx)?.is_canceled())
}
