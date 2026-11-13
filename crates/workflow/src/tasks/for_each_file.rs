//! `forEachFile` executor: selects files, runs the extracted callback on a
//! pool of threads, each with one sandbox worker (the JSSG runtime), and
//! commits the recorded file effects in one go.
//!
//! Callbacks may be async. Each worker keeps up to `concurrency` files in
//! flight, so callbacks that wait on the network overlap.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use codemod_deps::Registry;
use codemod_sandbox::sandbox::resolvers::OxcResolver;
use codemod_sandbox::WorkerOptions;
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use rquickjs::{async_with, CatchResultExt, Function, Module, Promise};
use serde::Deserialize;
use serde_json::{json, Value};

use super::changes::{self, is_safe_relative, PlannedChange};
use super::isolate::{self, module_name, StepIsolates};
use super::{block_on, TaskContext, TaskOutcome};
use crate::run::RunContext;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Selector {
    #[serde(default)]
    pub include: Vec<String>,
    #[serde(default)]
    pub exclude: Vec<String>,
    pub language: Option<String>,
    pub containing: Option<String>,
    /// Files each worker keeps in flight; more than one only helps callbacks
    /// that wait (network, `codemod:llm`).
    pub concurrency: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
pub(crate) enum Effect {
    Write { text: String },
    Rename { to: String },
    Delete,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileOutcome {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    effects: Vec<Effect>,
    #[serde(default)]
    output: Value,
    #[serde(default)]
    has_output: bool,
}

#[derive(Deserialize)]
struct Request {
    #[serde(default)]
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
    let (selector, files) = select_files(run, request.selector)?;
    let total = files.len();
    let processed = AtomicUsize::new(0);
    let report_every = (total / 100).max(1);
    let progress = || {
        let done = processed.fetch_add(1, Ordering::SeqCst) + 1;
        if done == total || done.is_multiple_of(report_every) {
            task.progress(done, total);
        }
    };

    let isolates = StepIsolates::new(task)?;
    let name = module_name(task, &request.callback);
    let next = AtomicUsize::new(0);
    let concurrency = selector.concurrency.unwrap_or(1).max(1);
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(files.len().div_ceil(concurrency))
        .max(1);
    let mut outcomes: Vec<Option<Result<FileOutcome, String>>> =
        (0..files.len()).map(|_| None).collect();

    let worker_results: Vec<WorkerResult> = if files.is_empty() {
        Vec::new()
    } else {
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    let options = isolates.worker_options(task, selector.language.clone());
                    let job = Job {
                        options,
                        registry: isolates.registry.clone(),
                        name: &name,
                        module,
                        captures: &captures,
                        language: selector.language.as_deref(),
                        files: &files,
                        next: &next,
                        concurrency,
                        cancel: &run.cancel,
                    };
                    let progress = &progress;
                    scope.spawn(move || worker(job, progress))
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
        })
    };
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
    let metrics = isolates.metrics(task);
    Ok(TaskOutcome::new(json!({
        "outputs": outputs,
        "changes": changes,
        "failures": failures,
        "metrics": metrics,
    }))
    .with_state(isolates.state_changes()))
}

/// The files a selector picks in the run's target, with the selector as
/// completed from the package's languages when it names none.
pub(crate) fn select_files(
    run: &RunContext,
    mut selector: Selector,
) -> Result<(Selector, Vec<(String, String)>), String> {
    let default_languages = if selector.include.is_empty() && selector.language.is_none() {
        package_languages(&run.bundle_dir)?
    } else {
        Vec::new()
    };
    if selector.language.is_none() && default_languages.len() == 1 {
        selector.language = default_languages.first().cloned();
    }
    let files = select(&run.target, &selector, &default_languages)?;
    Ok((selector, files))
}

/// One file a callback ran on, and what it did to it.
#[derive(Debug, Deserialize)]
pub(crate) struct FileEffects {
    pub path: String,
    pub original: String,
    #[serde(default)]
    pub effects: Vec<Effect>,
}

/// Fold each file's effects into planned changes and commit them. Files
/// whose effects are invalid become failures instead.
pub(crate) fn commit_effects(
    task: &TaskContext<'_>,
    files: Vec<FileEffects>,
) -> Result<(Value, Vec<Value>), String> {
    let mut planned = Vec::new();
    let mut failures = Vec::new();
    for file in files {
        let path = file.path.clone();
        match fold(file.path, file.original, file.effects) {
            Ok(plan) => planned.push(plan),
            Err(error) => failures.push(json!({ "path": path, "error": error })),
        }
    }
    let changes = changes::commit(task, &planned, task.run.dry_run)?;
    Ok((changes, failures))
}

/// The package's `targets.languages` from `codemod.yaml`, which select the
/// files when a `forEachFile` call gives no selector.
fn package_languages(bundle_dir: &Path) -> Result<Vec<String>, String> {
    let missing = || {
        "forEachFile needs a selector here: give it one (a glob, or { language }), or list the package's languages under `targets.languages` in codemod.yaml".to_string()
    };
    let manifest = bundle_dir.join("codemod.yaml");
    let Ok(text) = fs::read_to_string(&manifest) else {
        return Err(missing());
    };
    let manifest: serde_yaml::Value =
        serde_yaml::from_str(&text).map_err(|e| format!("{}: {e}", manifest.display()))?;
    let languages: Vec<String> = manifest
        .get("targets")
        .and_then(|targets| targets.get("languages"))
        .and_then(|languages| languages.as_sequence())
        .map(|languages| {
            languages
                .iter()
                .filter_map(|language| language.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if languages.is_empty() {
        return Err(missing());
    }
    Ok(languages)
}

/// Select files: globs and `.gitignore` first, then the text filter. Returns
/// paths relative to the target with their contents, sorted by path.
fn select(
    target: &Path,
    selector: &Selector,
    default_languages: &[String],
) -> Result<Vec<(String, String)>, String> {
    let include: Vec<String> = if !selector.include.is_empty() {
        selector.include.clone()
    } else {
        let languages: Vec<&str> = match selector.language.as_deref() {
            Some(language) => vec![language],
            None => default_languages.iter().map(String::as_str).collect(),
        };
        let mut globs = Vec::new();
        for language in languages {
            let extensions =
                extensions(language).ok_or_else(|| format!("unknown language '{language}'"))?;
            globs.extend(
                extensions
                    .iter()
                    .map(|extension| format!("**/*.{extension}")),
            );
        }
        globs
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

struct Job<'a> {
    options: WorkerOptions<OxcResolver>,
    registry: Option<Arc<dyn Registry>>,
    name: &'a str,
    module: &'a str,
    captures: &'a str,
    language: Option<&'a str>,
    files: &'a [(String, String)],
    next: &'a AtomicUsize,
    concurrency: usize,
    cancel: &'a Arc<AtomicBool>,
}

fn worker(job: Job<'_>, progress: &(dyn Fn() + Sync)) -> WorkerResult {
    let Job {
        options,
        registry,
        name,
        module,
        captures,
        language,
        files,
        next,
        concurrency,
        cancel,
    } = job;
    block_on(async move {
        let sandbox = isolate::start(options, registry).await?;
        let sandbox = &sandbox;
        async_with!(sandbox.context => |ctx| {
            let setup = async {
                let declared = Module::declare(ctx.clone(), name, module)?;
                let (evaluated, promise) = declared.eval()?;
                promise.into_future::<()>().await?;
                let init: Function = evaluated.get("__init")?;
                init.call::<_, ()>((captures, language))?;
                evaluated.get::<_, Function>("__runBatch")
            };
            let run_batch = setup
                .await
                .catch(&ctx)
                .map_err(|e| format!("could not load the callback: {e}"))?;

            let mut results = Vec::new();
            loop {
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                let start = next.fetch_add(concurrency, Ordering::SeqCst);
                let indices: Vec<usize> = (start..(start + concurrency).min(files.len())).collect();
                if indices.is_empty() {
                    break;
                }
                let batch: Vec<(&str, &str)> = indices
                    .iter()
                    .map(|&index| (files[index].0.as_str(), files[index].1.as_str()))
                    .collect();
                let batch = serde_json::to_string(&batch).map_err(|e| e.to_string())?;
                let settled = async {
                    let promise: Promise = run_batch.call((batch,))?;
                    promise.into_future::<String>().await
                };
                let outcomes: Result<Vec<FileOutcome>, String> = settled
                    .await
                    .catch(&ctx)
                    .map_err(|e| e.to_string())
                    .and_then(|json| serde_json::from_str(&json).map_err(|e| e.to_string()));
                let reported = sandbox.take_pending_failure().map(|failure| failure.message);
                let other_files = !sandbox.take_secondary_changes().is_empty();
                match outcomes {
                    Ok(outcomes) => {
                        for (index, outcome) in indices.iter().zip(outcomes) {
                            let outcome = if !outcome.ok {
                                Err(outcome.error.unwrap_or_else(|| "the callback failed".into()))
                            } else if let Some(message) = reported.as_ref().filter(|_| indices.len() == 1) {
                                Err(message.clone())
                            } else if other_files {
                                Err("jssgTransform() is not supported in forEachFile callbacks yet; change each file from its own callback".to_string())
                            } else {
                                Ok(outcome)
                            };
                            results.push((*index, outcome));
                            progress();
                        }
                    }
                    Err(error) => {
                        for index in &indices {
                            results.push((*index, Err(error.clone())));
                            progress();
                        }
                    }
                }
            }
            Ok(results)
        })
        .await
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
