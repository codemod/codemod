//! `forEachFile` executor: selects files, runs the extracted callback on a
//! pool of threads, each with one sandbox worker (the JSSG runtime), and
//! commits the recorded file effects in one go.
//!
//! Callbacks may be async. Each worker keeps up to `concurrency` files in
//! flight, so callbacks that wait on the network overlap.
//!
//! Selecting only walks the target: each worker reads its files as it gets
//! to them, and keeps a file's text only when the callback changed the file
//! (the commit needs it to check the file did not change on disk meanwhile,
//! and for the diff).

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use butterflow_core::execution::{self, language_globs};
use butterflow_core::jssg_runner::SemanticMode;
use codemod_deps::Registry;
use codemod_sandbox::sandbox::resolvers::OxcResolver;
use codemod_sandbox::WorkerOptions;
use rquickjs::{async_with, CatchResultExt, Function, Module, Promise};
use serde::Deserialize;
use serde_json::{json, Value};

use super::changes::{self, Change, PlannedChange};
use super::isolate::{self, module_name, StepIsolates};
use super::{block_on, parallel_map, TaskContext, TaskOutcome};
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
    /// that wait (network, `codemod/llm`).
    pub concurrency: Option<usize>,
    /// `"file"` (the default), `"workspace"` or `"none"`: what `definition()`
    /// and `references()` on roots `parse(file)` makes can find.
    #[serde(rename = "semanticAnalysis")]
    pub semantic_analysis: Option<Value>,
    /// Capabilities the callback needs besides the run's (`fs`, `fetch`,
    /// `child_process`).
    #[serde(default)]
    pub capabilities: Vec<String>,
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
    let (selector, paths) = select_paths(run, request.selector)?;
    let outcomes = run_callback(
        task,
        &request.callback,
        &request.captures,
        &selector,
        &paths,
    )?;

    let mut total = 0;
    let mut outputs = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new();
    let mut planned = Vec::new();
    for (path, outcome) in paths.into_iter().zip(outcomes) {
        let outcome = outcome.unwrap_or_else(|| Err("the file was not processed".to_string()));
        if matches!(outcome, Ok(Ran::Skipped)) {
            continue;
        }
        total += 1;
        match outcome {
            Ok(Ran::Skipped) => {}
            Err(error) => failures.push((path, error)),
            Ok(Ran::Done { outcome, original }) => {
                if outcome.has_output {
                    outputs.push(outcome.output);
                }
                // A file the callback left alone is not part of the commit, so
                // steps over other files may change it meanwhile.
                let Some(original) = original else {
                    continue;
                };
                match fold(path.clone(), original, outcome.effects) {
                    Ok(Some(plan)) => planned.push(plan),
                    Ok(None) => {}
                    Err(error) => failures.push((path, error)),
                }
            }
        }
    }

    if task.canceled() {
        return Err("canceled".to_string());
    }
    task.log_failures(&failures);
    every_file_failed(&failures, total)?;
    let changes = changes::commit(task, &planned, run.dry_run)?;
    let failed = failures.len();
    let failures: Vec<Value> = failures
        .into_iter()
        .map(|(path, error)| json!({ "path": path, "error": error }))
        .collect();
    Ok(TaskOutcome::new(json!({
        "outputs": outputs,
        "changes": changes,
        "failures": failures,
    }))
    .with_failures(failed, total))
}

/// What became of one selected file.
enum Ran {
    /// Not UTF-8, or without the selector's `containing` text: not one of
    /// the step's files.
    Skipped,
    /// The callback ran. `original` is the text it was given, kept only when
    /// it asked for changes.
    Done {
        outcome: FileOutcome,
        original: Option<String>,
    },
}

/// Run the extracted callback `callback` on the files at `paths` (relative to
/// the target), in workers, as the selector says (language, concurrency,
/// semantic analysis, capabilities, `containing`). Each worker reads its
/// files. Gives each file's outcome, in the order of `paths`; nothing is
/// committed.
fn run_callback(
    task: &TaskContext<'_>,
    callback: &str,
    captures: &Value,
    selector: &Selector,
    files: &[String],
) -> Result<Vec<Option<Result<Ran, String>>>, String> {
    let run = task.run;
    let module = run
        .callbacks
        .get(callback)
        .ok_or_else(|| format!("unknown callback '{callback}'"))?;
    let captures = serde_json::to_string(captures).map_err(|e| e.to_string())?;
    let total = files.len();
    let processed = AtomicUsize::new(0);
    let report_every = (total / 100).max(1);
    let progress = || {
        let done = processed.fetch_add(1, Ordering::SeqCst) + 1;
        if done == total || done.is_multiple_of(report_every) {
            task.progress(done, total);
        }
    };

    let mut isolates = StepIsolates::new(task)?;
    isolates.capabilities = isolate::capabilities(task, &selector.capabilities)?;
    let mode = isolate::semantic_mode(task, selector.semantic_analysis.as_ref())?;
    // Only a workspace-wide provider needs the files up front.
    let paths: Vec<_> = match mode {
        SemanticMode::Workspace(_) => files
            .iter()
            .map(|path| {
                let path = run.target.join(path);
                path.canonicalize().unwrap_or(path)
            })
            .collect(),
        _ => Vec::new(),
    };
    isolates.semantic_provider = butterflow_core::jssg_runner::semantic_provider(&mode, &paths);
    let name = module_name(task, callback);
    let next = AtomicUsize::new(0);
    let concurrency = selector.concurrency.unwrap_or(1).max(1);
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(files.len().div_ceil(concurrency))
        .max(1);
    let mut outcomes: Vec<Option<Result<Ran, String>>> = (0..files.len()).map(|_| None).collect();

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
                        target: &run.target,
                        files,
                        containing: selector.containing.as_deref(),
                        next: &next,
                        concurrency,
                        cancel: &task.cancel,
                        workflow_path: &run.workflow_path,
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
    Ok(outcomes)
}

/// A file's path, and what the callback returned for it or why it failed.
pub(crate) type FileOutput = (String, Result<Value, String>);

/// What the callback returned for each of the files at `paths` (relative to
/// the target; `null` for nothing), without committing what it did to them;
/// files it failed on are errors, files that are not UTF-8 are left out.
/// For `shard`'s filter.
pub(crate) fn callback_outputs(
    task: &TaskContext<'_>,
    callback: &str,
    captures: &Value,
    selector: &Selector,
    paths: &[String],
) -> Result<Vec<FileOutput>, String> {
    let outcomes = run_callback(task, callback, captures, selector, paths)?;
    Ok(paths
        .iter()
        .zip(outcomes)
        .filter_map(|(path, outcome)| {
            let output =
                match outcome.unwrap_or_else(|| Err("the file was not processed".to_string())) {
                    Ok(Ran::Skipped) => return None,
                    Ok(Ran::Done { outcome, .. }) => match outcome.error {
                        Some(error) if !outcome.ok => Err(error),
                        _ => Ok(outcome.output),
                    },
                    Err(error) => Err(error),
                };
            Some((path.clone(), output))
        })
        .collect())
}

/// A `forEachFile` step fails when its callback failed on every file it
/// ran on; when only some failed, it is partial.
pub(crate) fn every_file_failed(failures: &[(String, String)], total: usize) -> Result<(), String> {
    match failures.first() {
        Some((path, error)) if failures.len() == total => Err(format!(
            "the callback failed on every file; first error: {path}: {error}"
        )),
        _ => Ok(()),
    }
}

/// The files a selector picks in the run's target, with their text, and the
/// selector as completed from the package's languages when it names none.
pub(crate) fn select_files(
    run: &RunContext,
    selector: Selector,
) -> Result<(Selector, Vec<(String, String)>), String> {
    let (selector, paths) = select_paths(run, selector)?;
    let files = read_selected(&run.target, paths, selector.containing.as_deref());
    Ok((selector, files))
}

/// The paths (relative to the target) of the files the globs of a selector
/// pick in the run's target, unread, with the selector as completed from the
/// package's languages when it names none. Its `containing` filter is left
/// for when the files are read.
pub(crate) fn select_paths(
    run: &RunContext,
    mut selector: Selector,
) -> Result<(Selector, Vec<String>), String> {
    let default_languages = if selector.include.is_empty() && selector.language.is_none() {
        let languages = package_languages(&run.package_dir)?;
        if languages.is_empty() {
            return Err("forEachFile needs a selector here: give it one (a glob, or { language }), or list the package's languages under `targets.languages` in codemod.yaml".to_string());
        }
        languages
    } else {
        Vec::new()
    };
    if selector.language.is_none() && default_languages.len() == 1 {
        selector.language = default_languages.first().cloned();
    }
    let paths = select(&run.target, &selector, &default_languages)?;
    Ok((selector, paths))
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
            Ok(Some(plan)) => planned.push(plan),
            Ok(None) => {}
            Err(error) => failures.push(json!({ "path": path, "error": error })),
        }
    }
    let changes = changes::commit(task, &planned, task.run.dry_run)?;
    Ok((changes, failures))
}

/// The package's `targets.languages` from `codemod.yaml`: the default for a
/// `forEachFile` call that names no files or language. Empty without a
/// manifest, or when it lists none.
pub(crate) fn package_languages(bundle_dir: &Path) -> Result<Vec<String>, String> {
    let manifest = bundle_dir.join("codemod.yaml");
    let Ok(text) = fs::read_to_string(&manifest) else {
        return Ok(Vec::new());
    };
    let manifest: serde_yaml::Value =
        serde_yaml::from_str(&text).map_err(|e| format!("{}: {e}", manifest.display()))?;
    Ok(manifest
        .get("targets")
        .and_then(|targets| targets.get("languages"))
        .and_then(|languages| languages.as_sequence())
        .map(|languages| {
            languages
                .iter()
                .filter_map(|language| language.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}

/// Select files by globs and `.gitignore` (as `jssg` and `astGrep` select
/// theirs), without reading them. Returns paths relative to the target,
/// sorted. The selector's text filter is [`read_selected`]'s.
pub(crate) fn select(
    target: &Path,
    selector: &Selector,
    default_languages: &[String],
) -> Result<Vec<String>, String> {
    let include: Vec<String> = if !selector.include.is_empty() {
        selector.include.clone()
    } else {
        let languages: Vec<&str> = match selector.language.as_deref() {
            Some(language) => vec![language],
            None => default_languages.iter().map(String::as_str).collect(),
        };
        let mut globs = Vec::new();
        for language in languages {
            globs.extend(language_globs(language)?);
        }
        globs
    };

    execution::select_files(target, &include, &selector.exclude)?
        .into_iter()
        .map(|path| {
            Ok(path
                .strip_prefix(target)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/"))
        })
        .collect()
}

/// The text of a selected file (`path` relative to `target`), or `None` when
/// it is not one of the step's files: unreadable, not UTF-8, or without the
/// `containing` text.
fn read_selected_file(target: &Path, path: &str, containing: Option<&str>) -> Option<String> {
    let text = fs::read_to_string(target.join(path)).ok()?;
    match containing {
        Some(needle) if !text.contains(needle) => None,
        _ => Some(text),
    }
}

/// The selected files at `paths` (relative to `target`) with their text, read
/// in parallel; those [`read_selected_file`] leaves out are dropped.
pub(crate) fn read_selected(
    target: &Path,
    paths: Vec<String>,
    containing: Option<&str>,
) -> Vec<(String, String)> {
    let texts = parallel_map(&paths, |path| read_selected_file(target, path, containing));
    paths
        .into_iter()
        .zip(texts)
        .filter_map(|(path, text)| Some((path, text?)))
        .collect()
}

type WorkerResult = Result<Vec<(usize, Result<Ran, String>)>, String>;

struct Job<'a> {
    options: WorkerOptions<OxcResolver>,
    registry: Option<Arc<dyn Registry>>,
    name: &'a str,
    module: &'a str,
    captures: &'a str,
    language: Option<&'a str>,
    target: &'a Path,
    files: &'a [String],
    containing: Option<&'a str>,
    next: &'a AtomicUsize,
    concurrency: usize,
    cancel: &'a Arc<AtomicBool>,
    workflow_path: &'a Path,
}

fn worker(job: Job<'_>, progress: &(dyn Fn() + Sync)) -> WorkerResult {
    let Job {
        options,
        registry,
        name,
        module,
        captures,
        language,
        target,
        files,
        containing,
        next,
        concurrency,
        cancel,
        workflow_path,
    } = job;
    block_on(async move {
        let sandbox = isolate::start(options, registry).await?;
        let sandbox = &sandbox;
        async_with!(sandbox.context => |ctx| {
            let setup = async {
                let declared = Module::declare(ctx.clone(), name, module)?;
                crate::run::set_import_meta(&declared, workflow_path)?;
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
                let range = start..(start + concurrency).min(files.len());
                if range.is_empty() {
                    break;
                }
                // Read this batch's files; the ones that are not the step's
                // are done.
                let mut indices = Vec::with_capacity(range.len());
                let mut texts = Vec::with_capacity(range.len());
                for index in range {
                    match read_selected_file(target, &files[index], containing) {
                        Some(text) => {
                            indices.push(index);
                            texts.push(text);
                        }
                        None => {
                            results.push((index, Ok(Ran::Skipped)));
                            progress();
                        }
                    }
                }
                if indices.is_empty() {
                    continue;
                }
                let settled = async {
                    let paths: Vec<&str> = indices.iter().map(|&index| files[index].as_str()).collect();
                    let batch: Vec<&str> = texts.iter().map(String::as_str).collect();
                    let promise: Promise = run_batch.call((paths, batch))?;
                    promise.into_future::<String>().await
                };
                let outcomes: Result<Vec<FileOutcome>, String> = settled
                    .await
                    .catch(&ctx)
                    .map_err(|e| e.to_string())
                    .and_then(|json| serde_json::from_str(&json).map_err(|e| e.to_string()));
                let effects = sandbox.take_side_effects();
                let reported = effects.failure.map(|failure| failure.message);
                let other_files = !effects.other_files.is_empty();
                match outcomes {
                    Ok(outcomes) => {
                        for ((index, outcome), text) in indices.iter().zip(outcomes).zip(texts) {
                            let outcome = if !outcome.ok {
                                Err(outcome.error.unwrap_or_else(|| "the callback failed".into()))
                            } else if let Some(message) = reported.as_ref().filter(|_| indices.len() == 1) {
                                Err(message.clone())
                            } else if other_files {
                                Err("jssgTransform() is not supported in forEachFile callbacks yet; change each file from its own callback".to_string())
                            } else {
                                // The text is only needed to commit changes.
                                let original = (!outcome.effects.is_empty()).then_some(text);
                                Ok(Ran::Done { outcome, original })
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

/// What a callback's effects leave of one file. `delete()` wins over
/// whatever else the callback did; the last `write()` and `rename()` count.
/// `None` when they leave it as it was (it was written its own text).
fn fold(
    path: String,
    original: String,
    effects: Vec<Effect>,
) -> Result<Option<PlannedChange>, String> {
    let mut text = None;
    let mut rename_to = None;
    let mut deleted = false;
    for effect in effects {
        match effect {
            Effect::Write { text: next } => text = Some(next),
            Effect::Rename { to } => {
                rename_to = Some(changes::normalize(&to).ok_or_else(|| {
                    format!(
                        "cannot rename to '{to}': the path must be relative and stay inside the target"
                    )
                })?);
            }
            Effect::Delete => deleted = true,
        }
    }
    if deleted {
        return Ok(Some(PlannedChange {
            path,
            original,
            change: Change::Delete,
        }));
    }
    let plan = match text {
        Some(text) => PlannedChange::moved(path, original, text, rename_to),
        None => {
            let text = original.clone();
            PlannedChange::moved(path, original, text, rename_to)
        }
    };
    Ok(match &plan.change {
        Change::Edit(text) if *text == plan.original => None,
        _ => Some(plan),
    })
}
