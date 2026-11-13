//! `dependencyGraph({ lockfiles }, async (g) => ...)`: reads the target's
//! lockfiles into one graph and runs the extracted callback in its own
//! isolate with `g`, a live handle on it. `g` never leaves Rust: its methods
//! are host functions over node ids, and only what the callback returns is
//! recorded.

use std::collections::HashSet;
use std::sync::Arc;

use codemod_deps::{Kind, Walk};
use rquickjs::{async_with, CatchResultExt, Ctx, Function, Module, Object, Promise};
use serde::Deserialize;
use serde_json::{json, Value};

use super::isolate::{self, module_name, StepIsolates};
use super::{block_on, Globs, TaskContext, TaskOutcome};
use crate::deps_host::DEPENDENCIES_MODULE;

#[derive(Deserialize)]
struct Request {
    #[serde(default)]
    lockfiles: Option<Globs>,
    callback: String,
    captures: Value,
}

#[derive(Deserialize)]
struct Outcome {
    ok: bool,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    error: Option<String>,
}

pub(crate) fn run(task: &TaskContext<'_>, request: Value) -> Result<TaskOutcome, String> {
    let request: Request =
        serde_json::from_value(request).map_err(|e| format!("bad dependencyGraph task: {e}"))?;
    let run = task.run;
    let module = run
        .callbacks
        .get(&request.callback)
        .ok_or_else(|| format!("unknown callback '{}'", request.callback))?;
    let only = request.lockfiles.map(Globs::into_vec).unwrap_or_default();
    let graph = codemod_deps::load(&run.target, &only)?;
    task.log(format!(
        "read {} ({} packages, {} dependencies)",
        graph.lockfiles.join(", "),
        graph.nodes.len(),
        graph.edges.len()
    ));
    let walk = Arc::new(Walk::new(graph));
    let captures = serde_json::to_string(&request.captures).map_err(|e| e.to_string())?;
    let isolates = StepIsolates::new(task)?;
    let options = isolates.worker_options(task, None);
    let registry = isolates.registry.clone();
    let name = module_name(task, &request.callback);
    // Gives the callback module's `__runGraph` its graph.
    let attach = format!(
        "import {{ __graph }} from {DEPENDENCIES_MODULE:?};\nglobalThis.__dependencyGraph = __graph();\n"
    );
    let attach_name = module_name(task, "dependency-graph");

    let json = block_on(async move {
        let sandbox = isolate::start(options, registry).await?;
        let sandbox = &sandbox;
        async_with!(sandbox.context => |ctx| {
            let run = async {
                install_graph(&ctx, walk)?;
                let (_, promise) = Module::declare(ctx.clone(), attach_name.as_str(), attach.as_str())?.eval()?;
                promise.into_future::<()>().await?;
                let declared = Module::declare(ctx.clone(), name.as_str(), module.as_str())?;
                let (evaluated, promise) = declared.eval()?;
                promise.into_future::<()>().await?;
                let init: Function = evaluated.get("__init")?;
                init.call::<_, ()>((captures.as_str(), None::<&str>))?;
                let run_graph: Function = evaluated.get("__runGraph")?;
                let promise: Promise = run_graph.call(())?;
                promise.into_future::<String>().await
            };
            run.await.catch(&ctx).map_err(|e| e.to_string())
        })
        .await
    })?;

    let outcome: Outcome = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    isolates.metrics(task);
    if !outcome.ok {
        return Err(outcome
            .error
            .unwrap_or_else(|| "the step failed".to_string()));
    }
    Ok(TaskOutcome::new(outcome.result).with_state(isolates.state_changes()))
}

/// `__graphHost`: the graph's host functions, over node ids and JSON.
fn install_graph(ctx: &Ctx<'_>, walk: Arc<Walk>) -> rquickjs::Result<()> {
    let host = Object::new(ctx.clone())?;
    host.set("lockfiles", json!(walk.graph.lockfiles).to_string())?;

    let graph = Arc::clone(&walk);
    host.set(
        "node",
        Function::new(ctx.clone(), move |id: usize| {
            node_json(&graph, id).to_string()
        })?,
    )?;

    let graph = Arc::clone(&walk);
    host.set(
        "nodes",
        Function::new(ctx.clone(), move |which: String, name: String| {
            let ids: Vec<usize> = match which.as_str() {
                "workspaces" => graph.workspaces(),
                "named" => graph.nodes_named(&name).to_vec(),
                _ => (0..graph.graph.nodes.len()).collect(),
            };
            Value::Array(ids.into_iter().map(|id| node_json(&graph, id)).collect()).to_string()
        })?,
    )?;

    let graph = Arc::clone(&walk);
    host.set(
        "edges",
        Function::new(
            ctx.clone(),
            move |direction: String, key: String, kinds: String| -> rquickjs::Result<String> {
                let kinds = parse_kinds(&kinds)?;
                let id = || key.parse::<usize>().unwrap_or(usize::MAX);
                let edges = match direction.as_str() {
                    "out" if id() < graph.graph.nodes.len() => graph.dependencies(id(), &kinds),
                    "in" if id() < graph.graph.nodes.len() => graph.dependents(id(), &kinds),
                    "named" => graph.declaring(&key, &kinds),
                    _ => Vec::new(),
                };
                let edges: Vec<Value> = edges
                    .into_iter()
                    .map(|index| {
                        let edge = &graph.graph.edges[index];
                        json!({
                            "from": edge.from,
                            "to": edge.to,
                            "name": edge.name,
                            "range": edge.range,
                            "kind": edge.kind,
                        })
                    })
                    .collect();
                Ok(Value::Array(edges).to_string())
            },
        )?,
    )?;

    let graph = Arc::clone(&walk);
    host.set(
        "chains",
        Function::new(
            ctx.clone(),
            move |from: String, to: String, kinds: String| -> rquickjs::Result<String> {
                let ids = |text: &str| -> Vec<usize> {
                    serde_json::from_str::<Vec<usize>>(text)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|&id| id < graph.graph.nodes.len())
                        .collect()
                };
                let to: HashSet<usize> = ids(&to).into_iter().collect();
                let chains = graph.chains(&ids(&from), &to, &parse_kinds(&kinds)?);
                Ok(json!(chains).to_string())
            },
        )?,
    )?;

    ctx.globals().set("__graphHost", host)
}

fn node_json(walk: &Walk, id: usize) -> Value {
    let Some(node) = walk.graph.nodes.get(id) else {
        return Value::Null;
    };
    json!({
        "id": id,
        "ecosystem": node.ecosystem,
        "name": node.name,
        "version": node.version,
        "workspace": node.workspace,
    })
}

/// Comma-separated kinds; empty means every kind.
fn parse_kinds(text: &str) -> rquickjs::Result<Vec<Kind>> {
    text.split(',')
        .filter(|kind| !kind.is_empty())
        .map(|kind| {
            Kind::parse(kind).ok_or_else(|| {
                rquickjs::Error::new_from_js_message(
                    "string",
                    "kind",
                    format!(
                        "unknown dependency kind '{kind}'; use prod, dev, peer, optional or build"
                    ),
                )
            })
        })
        .collect()
}
