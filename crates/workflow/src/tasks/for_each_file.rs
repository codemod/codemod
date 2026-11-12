//! `forEachFile` executor: selects files, runs the extracted callback on a
//! pool of threads (one QuickJS context per thread), and commits the
//! recorded file effects in one go.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use codemod_sandbox::AstGrepModule;
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use rquickjs::loader::{BuiltinLoader, BuiltinResolver, ModuleLoader};
use rquickjs::{CatchResultExt, Context, Function, Module, Runtime};
use serde::Deserialize;
use serde_json::{json, Value};

use super::changes::{self, is_safe_relative, PlannedChange};
use super::{TaskContext, TaskOutcome};

const CALLBACK_MODULE: &str = "callback.js";

#[derive(Debug, Clone, Deserialize)]
pub struct Selector {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    pub language: Option<String>,
    pub containing: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum Effect {
    Write { text: String },
    Rename { to: String },
    Delete,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileOutcome {
    effects: Vec<Effect>,
    output: Value,
    has_output: bool,
}

#[derive(Deserialize)]
struct Request {
    selector: Selector,
    callback: String,
    captures: Value,
}

pub(crate) fn run(task: &TaskContext<'_>, request: Value) -> Result<TaskOutcome, String> {
    let request: Request =
        serde_json::from_value(request).map_err(|e| format!("bad forEachFile task: {e}"))?;
    let run = task.run;
    let module = run
        .callbacks
        .get(&request.callback)
        .ok_or_else(|| format!("unknown callback '{}'", request.callback))?;
    let captures = serde_json::to_string(&request.captures).map_err(|e| e.to_string())?;
    let files = select(&run.target, &request.selector)?;
    let total = files.len();
    let processed = AtomicUsize::new(0);
    let report_every = (total / 100).max(1);
    let progress = || {
        let done = processed.fetch_add(1, Ordering::SeqCst) + 1;
        if done == total || done.is_multiple_of(report_every) {
            task.progress(done, total);
        }
    };

    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(files.len())
        .max(1);
    let mut outcomes: Vec<Option<Result<FileOutcome, String>>> =
        (0..files.len()).map(|_| None).collect();

    let worker_results: Vec<WorkerResult> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    worker(
                        Worker {
                            module,
                            captures: &captures,
                            language: request.selector.language.as_deref(),
                            files: &files,
                            next: &next,
                            cancel: &run.cancel,
                        },
                        &progress,
                    )
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|_| Err("a forEachFile worker panicked".to_string()))
            })
            .collect()
    });
    for result in worker_results {
        for (index, outcome) in result? {
            outcomes[index] = Some(outcome);
        }
    }

    let mut outputs = Vec::new();
    let mut failures = Vec::new();
    let mut planned = Vec::new();
    for ((path, original), outcome) in files.into_iter().zip(outcomes) {
        match outcome.unwrap_or_else(|| Err("the file was not processed".to_string())) {
            Err(error) => failures.push(json!({ "path": path, "error": error })),
            Ok(outcome) => {
                if outcome.has_output {
                    outputs.push(outcome.output);
                }
                match fold(path.clone(), original, outcome.effects) {
                    Ok(plan) => planned.push(plan),
                    Err(error) => failures.push(json!({ "path": path, "error": error })),
                }
            }
        }
    }

    if run.canceled() {
        return Err("canceled".to_string());
    }
    let changes = changes::commit(task, &planned, run.dry_run)?;
    Ok(TaskOutcome::new(
        json!({ "outputs": outputs, "changes": changes, "failures": failures }),
    ))
}

/// Select files: globs and `.gitignore` first, then the text filter. Returns
/// paths relative to the target with their contents, sorted by path.
fn select(target: &Path, selector: &Selector) -> Result<Vec<(String, String)>, String> {
    let include: Vec<String> = if selector.include.is_empty() {
        let language = selector.language.as_deref().unwrap_or_default();
        extensions(language)
            .ok_or_else(|| format!("unknown language '{language}'"))?
            .iter()
            .map(|extension| format!("**/*.{extension}"))
            .collect()
    } else {
        selector.include.clone()
    };

    let mut overrides = OverrideBuilder::new(target);
    for glob in &include {
        overrides
            .add(glob)
            .map_err(|e| format!("bad glob '{glob}': {e}"))?;
    }
    for glob in &selector.exclude {
        overrides
            .add(&format!("!{glob}"))
            .map_err(|e| format!("bad glob '{glob}': {e}"))?;
    }
    let overrides = overrides.build().map_err(|e| e.to_string())?;

    let mut files = Vec::new();
    for entry in WalkBuilder::new(target)
        .overrides(overrides)
        .require_git(false)
        .build()
    {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(target)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue; // not UTF-8
        };
        if let Some(needle) = &selector.containing {
            if !text.contains(needle.as_str()) {
                continue;
            }
        }
        files.push((relative, text));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

type WorkerResult = Result<Vec<(usize, Result<FileOutcome, String>)>, String>;

struct Worker<'a> {
    module: &'a str,
    captures: &'a str,
    language: Option<&'a str>,
    files: &'a [(String, String)],
    next: &'a AtomicUsize,
    cancel: &'a Arc<AtomicBool>,
}

fn worker(job: Worker<'_>, progress: &(dyn Fn() + Sync)) -> WorkerResult {
    let Worker {
        module,
        captures,
        language,
        files,
        next,
        cancel,
    } = job;
    let runtime = Runtime::new().map_err(|e| e.to_string())?;
    let interrupt = Arc::clone(cancel);
    runtime.set_interrupt_handler(Some(Box::new(move || interrupt.load(Ordering::SeqCst))));
    let context = Context::full(&runtime).map_err(|e| e.to_string())?;
    runtime.set_loader(
        (
            BuiltinResolver::default().with_module("codemod:ast-grep"),
            BuiltinResolver::default().with_module(CALLBACK_MODULE),
        ),
        (
            ModuleLoader::default().with_module("codemod:ast-grep", AstGrepModule),
            BuiltinLoader::default().with_module(CALLBACK_MODULE, module),
        ),
    );

    context.with(|ctx| {
        let setup =
            || -> rquickjs::Result<(Function<'_>, Module<'_, rquickjs::module::Evaluated>)> {
                let declared = Module::declare(ctx.clone(), CALLBACK_MODULE, module)?;
                let (evaluated, promise) = declared.eval()?;
                promise.finish::<()>()?;
                let init: Function = evaluated.get("__init")?;
                init.call::<_, ()>((captures, language))?;
                let run: Function = evaluated.get("__run")?;
                Ok((run, evaluated))
            };
        let (run, _module) = setup()
            .catch(&ctx)
            .map_err(|e| format!("could not load the callback: {e}"))?;

        let mut results = Vec::new();
        loop {
            if cancel.load(Ordering::SeqCst) {
                break;
            }
            let index = next.fetch_add(1, Ordering::SeqCst);
            let Some((path, text)) = files.get(index) else {
                break;
            };
            let outcome = run
                .call::<_, String>((path.as_str(), text.as_str()))
                .catch(&ctx)
                .map_err(|e| e.to_string())
                .and_then(|json| {
                    serde_json::from_str::<FileOutcome>(&json).map_err(|e| e.to_string())
                });
            results.push((index, outcome));
            progress();
        }
        Ok(results)
    })
}

fn fold(path: String, original: String, effects: Vec<Effect>) -> Result<PlannedChange, String> {
    let mut plan = PlannedChange {
        text: original.clone(),
        path,
        original,
        rename_to: None,
        delete: false,
    };
    for effect in effects {
        match effect {
            Effect::Write { text } => plan.text = text,
            Effect::Rename { to } => {
                if !is_safe_relative(&to) {
                    return Err(format!(
                        "cannot rename to '{to}': the path must be relative and stay inside the target"
                    ));
                }
                plan.rename_to = Some(to.replace('\\', "/"));
            }
            Effect::Delete => plan.delete = true,
        }
    }
    if plan.rename_to.as_deref() == Some(plan.path.as_str()) {
        plan.rename_to = None;
    }
    Ok(plan)
}

fn extensions(language: &str) -> Option<&'static [&'static str]> {
    Some(match language {
        "typescript" => &["ts", "mts", "cts"],
        "tsx" => &["tsx"],
        "javascript" => &["js", "mjs", "cjs", "jsx"],
        "css" => &["css"],
        "json" => &["json"],
        "html" => &["html"],
        "python" => &["py"],
        "rust" => &["rs"],
        "go" => &["go"],
        "java" => &["java"],
        "yaml" => &["yaml", "yml"],
        _ => return None,
    })
}
