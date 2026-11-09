//! The `quickjs` orchestration runtime's host: ONE QuickJS runtime and ONE
//! context run a whole prebuilt workflow bundle (`codemod-workflow build
//! --runtime=quickjs`): the portable TypeScript control plane (run, replay,
//! history, scheduling), the workflow body, and every JSSG transform.
//!
//! The host provides, before the bundle is evaluated:
//!
//! - the transform built-ins (`codemod:ast-grep` over the shared sg_core,
//!   `codemod:workflow`, `codemod:metrics`, `codemod:runtime`, the curated
//!   `fs`, LLRT's safe modules and globals such as `AbortController` and
//!   timers), from `codemod_sandbox::sandbox::engine::embedded`;
//! - `globalThis.__codemodQuickjsNative` ([`native`]): synchronous file
//!   system calls with POSIX error codes, environment, clock, process spawn
//!   with streaming output and process-group kill, console output, a
//!   virtual-module registry for transform artifacts, and the per-file JSSG
//!   calls ([`jssg`]) that parse a file into an `SgRoot` before the
//!   transform and convert its result into a `FileOutcome` after it, with
//!   the batch rules the YAML engine's JSSG step shares (`crates/jssg-batch`).
//!
//! The bundle's entry builds the orchestration `Host` from those primitives
//! and exports `main(request)`, which resolves to the run's JSON result.
//! JavaScript runs on exactly one thread; child-process pipes are drained by
//! helper threads.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use codemod_sandbox::sandbox::engine::embedded::{configure_runtime, install_context};
use rquickjs::{
    async_with, AsyncContext, AsyncRuntime, CatchResultExt, Function, Module, Object, Promise,
};

pub mod http;
pub mod jssg;
pub mod modules;
pub mod native;
pub mod pool;
pub mod select;
mod utf8;

/// Module name of the workflow bundle inside the runtime.
pub const BUNDLE_MODULE: &str = "codemod-workflow-bundle.mjs";

/// Where the bundle's entry publishes the function that aborts its run.
pub const ABORT_GLOBAL: &str = "__codemodQuickjsAbort";

pub struct HostOptions {
    /// The prebuilt ES module bundle.
    pub bundle: PathBuf,
    /// Repository root: JSSG target root and `shell` cwd.
    pub target: PathBuf,
    /// The root input as JSON text (`None`: no input).
    pub input: Option<String>,
    /// History JSON to resume from (text of a `History`), if any.
    pub history: Option<String>,
}

/// What the bundle's `main` resolved to: a JSON object with `ok`, then
/// `output` (success) or `error` (failure), plus `history` and, when
/// benchmark timings are on, `timings` (the JS-side phases, in the timings
/// schema's `phases` shape).
pub struct HostOutcome {
    pub result: serde_json::Value,
}

/// Benchmark timings: the bundle's JS-side phases (`result.timings`, the
/// schema's `phases` shape) into the calling thread's accumulators.
pub fn record_js_timings(result: &serde_json::Value) {
    let Some(phases) = result
        .get("timings")
        .and_then(|timings| timings.as_object())
    else {
        return;
    };
    for (name, phase) in phases {
        let Some(phase_id) = bench_timings::Phase::from_name(name) else {
            continue;
        };
        let field = |key: &str| {
            phase
                .get(key)
                .and_then(|value| value.as_f64())
                .unwrap_or(0.0)
        };
        bench_timings::add(phase_id, field("ns") as u64, field("count") as u64);
        bench_timings::add_cpu(phase_id, field("cpu_ns") as u64);
    }
}

/// Run the bundle to completion on a current-thread tokio runtime.
pub fn run(options: HostOptions, started: Instant) -> Result<HostOutcome, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|error| format!("failed to start the async runtime: {error}"))?;
    runtime.block_on(run_async(options, started))
}

async fn run_async(options: HostOptions, started: Instant) -> Result<HostOutcome, String> {
    use bench_timings::Phase;
    // Benchmark timings: process start-up, the runtime, and the context are
    // `boot`; reading, compiling, and evaluating the bundle is `plan`.
    bench_timings::add(Phase::Boot, bench_timings::since_start_ns(), 1);
    let plan = bench_timings::start();
    let source = std::fs::read_to_string(&options.bundle).map_err(|error| {
        format!(
            "failed to read bundle {}: {error}",
            options.bundle.display()
        )
    })?;
    let target = options
        .target
        .canonicalize()
        .map_err(|error| format!("target {}: {error}", options.target.display()))?;
    bench_timings::stop(Phase::Plan, plan);

    let boot = bench_timings::start();
    let runtime = AsyncRuntime::new().map_err(|error| format!("QuickJS runtime: {error}"))?;
    let registry = modules::ModuleRegistry::default();
    let globals = configure_runtime(&runtime, registry.resolver(), registry.loader()).await;
    let context = AsyncContext::full(&runtime)
        .await
        .map_err(|error| format!("QuickJS context: {error}"))?;

    let input = options.input;
    let history = options.history;
    let result: Result<String, String> = async_with!(context => |ctx| {
        install_context(&ctx, globals, &target).map_err(|error| error.to_string())?;
        let state = Rc::new(native::HostState {
            started,
            registry: registry.clone(),
            batches: RefCell::default(),
            children: RefCell::default(),
            next_id: Default::default(),
            target: target.clone(),
            pool: RefCell::default(),
        });
        native::install(&ctx, state).map_err(|error| format!("failed to install host: {error}"))?;
        bench_timings::stop(Phase::Boot, boot);

        let plan = bench_timings::start();
        let module = Module::declare(ctx.clone(), BUNDLE_MODULE, source)
            .catch(&ctx)
            .map_err(|error| format!("failed to compile the bundle: {error}"))?;
        let (module, evaluated) = module
            .eval()
            .catch(&ctx)
            .map_err(|error| format!("failed to evaluate the bundle: {error}"))?;
        evaluated
            .into_future::<()>()
            .await
            .catch(&ctx)
            .map_err(|error| format!("failed to evaluate the bundle: {error}"))?;
        bench_timings::stop(Phase::Plan, plan);

        let main: Function = module
            .get("main")
            .catch(&ctx)
            .map_err(|error| format!("the bundle exports no main(): {error}"))?;
        let request = Object::new(ctx.clone()).map_err(|error| error.to_string())?;
        request
            .set("target", target.to_string_lossy().as_ref())
            .map_err(|error| error.to_string())?;
        if let Some(input) = &input {
            request.set("input", input.as_str()).map_err(|error| error.to_string())?;
        }
        if let Some(history) = &history {
            request.set("history", history.as_str()).map_err(|error| error.to_string())?;
        }

        // SIGINT/SIGTERM/SIGHUP abort the run (the bundle's entry publishes
        // the abort function); children run in their own process groups, so
        // this is how a terminal's signal reaches them.
        {
            let ctx = ctx.clone();
            let mut signals = Vec::new();
            for kind in [
                tokio::signal::unix::SignalKind::interrupt(),
                tokio::signal::unix::SignalKind::terminate(),
                tokio::signal::unix::SignalKind::hangup(),
            ] {
                if let Ok(signal) = tokio::signal::unix::signal(kind) {
                    signals.push(signal);
                }
            }
            ctx.clone().spawn(async move {
                if signals.is_empty() {
                    return;
                }
                std::future::poll_fn(|cx| {
                    for signal in signals.iter_mut() {
                        if signal.poll_recv(cx).is_ready() {
                            return std::task::Poll::Ready(());
                        }
                    }
                    std::task::Poll::Pending
                })
                .await;
                if let Ok(abort) = ctx.globals().get::<_, Function>(ABORT_GLOBAL) {
                    if abort.call::<_, ()>(()).is_err() {
                        let _ = ctx.catch();
                    }
                }
            });
        }

        let pending: Promise = main
            .call((request,))
            .catch(&ctx)
            .map_err(|error| format!("main() failed: {error}"))?;
        pending
            .into_future::<String>()
            .await
            .catch(&ctx)
            .map_err(|error| format!("main() failed: {error}"))
    })
    .await;
    // Dropping the context and the runtime (garbage collection of every
    // object the run left) is `setup`.
    let teardown = bench_timings::start();
    drop(context);
    drop(runtime);
    bench_timings::stop(Phase::Setup, teardown);
    let text = result?;
    let result = serde_json::from_str(&text)
        .map_err(|error| format!("main() returned invalid JSON: {error}"))?;
    Ok(HostOutcome { result })
}
