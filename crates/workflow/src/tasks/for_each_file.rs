//! `forEachFile` executor: selects files, runs the extracted callback on a
//! pool of threads, each with one sandbox worker (the JSSG runtime), and
//! commits the recorded file effects in one go.
//!
//! Callbacks may be async. Each worker keeps up to `concurrency` files in
//! flight, so callbacks that wait on the network overlap.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use butterflow_core::execution::{self, language_globs};
use butterflow_core::jssg_runner::JssgFileChange;
use codemod_deps::Registry;
use codemod_sandbox::sandbox::engine::{
    ExecutionResult, FileChange, ModifiedResult, PendingFiles, TakenFile, Transaction,
};
use codemod_sandbox::sandbox::resolvers::OxcResolver;
use codemod_sandbox::WorkerOptions;
use rquickjs::{async_with, CatchResultExt, Ctx, Exception, Function, Module, Object, Promise};
use serde::Deserialize;
use serde_json::{json, Value};

use super::changes::{self, Change, PlannedChange};
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
    let (selector, files) = select_files(run, request.selector)?;
    let total = files.len();
    let (outcomes, other_changes) = run_callback(
        task,
        &request.callback,
        &request.captures,
        &selector,
        &files,
    )?;

    let mut outputs = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new();
    let mut planned = Vec::new();
    for ((path, original), outcome) in files.into_iter().zip(outcomes) {
        match outcome.unwrap_or_else(|| Err("the file was not processed".to_string())) {
            Err(error) => failures.push((path, error)),
            Ok(outcome) => {
                if outcome.has_output {
                    outputs.push(outcome.output);
                }
                // A file the callback left alone is not part of the commit, so
                // steps over other files may change it meanwhile.
                if outcome.effects.is_empty() {
                    continue;
                }
                match fold(path.clone(), original, outcome.effects) {
                    Ok(plan) => planned.push(plan),
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
    planned.extend(changes::plan(task, &other_changes)?);
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

/// What a callback did to each of a step's files, in their order (`None`
/// for a file it did not get to), and the files jssg transforms changed
/// through the step's unwritten files (see [`OtherFiles`]).
type CallbackOutcomes = (
    Vec<Option<Result<FileOutcome, String>>>,
    Vec<JssgFileChange>,
);

/// Run the extracted callback `callback` on `files`, in workers, as the
/// selector says (language, concurrency, semantic analysis, capabilities).
/// Gives each file's outcome, in the order of `files`; nothing is committed.
fn run_callback(
    task: &TaskContext<'_>,
    callback: &str,
    captures: &Value,
    selector: &Selector,
    files: &[(String, String)],
) -> Result<CallbackOutcomes, String> {
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
    let paths: Vec<_> = files
        .iter()
        .map(|(path, _)| {
            let path = run.target.join(path);
            path.canonicalize().unwrap_or(path)
        })
        .collect();
    isolates.semantic_provider = butterflow_core::jssg_runner::semantic_provider(&mode, &paths);
    let name = module_name(task, callback);
    let next = AtomicUsize::new(0);
    let concurrency = selector.concurrency.unwrap_or(1).max(1);
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(files.len().div_ceil(concurrency))
        .max(1);
    let mut outcomes: Vec<Option<Result<FileOutcome, String>>> =
        (0..files.len()).map(|_| None).collect();
    let other_files = OtherFiles::default();

    let worker_results: Vec<WorkerResult> = if files.is_empty() {
        Vec::new()
    } else {
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    let mut options = isolates.worker_options(task, selector.language.clone());
                    options.pending_files = Some(other_files.pending.clone());
                    let job = Job {
                        options,
                        registry: isolates.registry.clone(),
                        name: &name,
                        module,
                        captures: &captures,
                        language: selector.language.as_deref(),
                        files,
                        next: &next,
                        concurrency,
                        cancel: &task.cancel,
                        workflow_path: &run.workflow_path,
                        target: &run.target,
                        other_files: &other_files,
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
    Ok((outcomes, other_files.into_changes()))
}

/// A file's path, and what the callback returned for it or why it failed.
pub(crate) type FileOutput = (String, Result<Value, String>);

/// What the callback returned for each of `files` (`null` for nothing),
/// without committing what it did to them; files it failed on are errors.
/// For `shard`'s filter.
pub(crate) fn callback_outputs(
    task: &TaskContext<'_>,
    callback: &str,
    captures: &Value,
    selector: &Selector,
    files: &[(String, String)],
) -> Result<Vec<FileOutput>, String> {
    // A filter changes nothing: what transforms changed is dropped.
    let (outcomes, _) = run_callback(task, callback, captures, selector, files)?;
    Ok(files
        .iter()
        .zip(outcomes)
        .map(|((path, _), outcome)| {
            let output = outcome
                .unwrap_or_else(|| Err("the file was not processed".to_string()))
                .and_then(|outcome| match outcome.error {
                    Some(error) if !outcome.ok => Err(error),
                    _ => Ok(outcome.output),
                });
            (path.clone(), output)
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

/// The files a selector picks in the run's target, with the selector as
/// completed from the package's languages when it names none.
pub(crate) fn select_files(
    run: &RunContext,
    mut selector: Selector,
) -> Result<(Selector, Vec<(String, String)>), String> {
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

/// Select files: globs and `.gitignore` first (as `jssg` and `astGrep`
/// select theirs), then the text filter. Returns paths relative to the
/// target with their contents, sorted by path.
pub(crate) fn select(
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
            globs.extend(language_globs(language)?);
        }
        globs
    };

    let mut files = Vec::new();
    for path in execution::select_files(target, &include, &selector.exclude)? {
        let relative = path
            .strip_prefix(target)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let Ok(text) = fs::read_to_string(&path) else {
            continue; // not UTF-8
        };
        if let Some(needle) = &selector.containing {
            if !text.contains(needle.as_str()) {
                continue;
            }
        }
        files.push((relative, text));
    }
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
    workflow_path: &'a Path,
    target: &'a Path,
    other_files: &'a OtherFiles,
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
        workflow_path,
        target,
        other_files,
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

            // One file at a time per worker: a jssg transform the YAML
            // translation runs may change other files with `jssgTransform`.
            let slot: CompatSlot = Arc::default();
            if concurrency == 1 {
                install_compat_host(&ctx, other_files.pending.clone(), target.to_path_buf(), Arc::clone(&slot))
                    .map_err(|e| format!("could not start the step's runtime: {e}"))?;
            }

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
                let mut attempts = 0;
                let outcomes = loop {
                    let transaction = (concurrency == 1).then(|| other_files.pending.begin());
                    let settled = async {
                        let promise: Promise = run_batch.call((batch.as_str(),))?;
                        promise.into_future::<String>().await
                    };
                    let outcomes: Result<Vec<FileOutcome>, String> = settled
                        .await
                        .catch(&ctx)
                        .map_err(|e| e.to_string())
                        .and_then(|json| serde_json::from_str(&json).map_err(|e| e.to_string()));
                    let effects = sandbox.take_side_effects();
                    let compat = slot.lock().unwrap_or_else(|e| e.into_inner()).take();
                    // It gave way to another file's transform that was waiting
                    // for a file it held, so what it did is incomplete: drop it
                    // and run it again once that one is done.
                    if transaction.as_ref().is_some_and(Transaction::aborted) {
                        drop(compat);
                        if let Some(transaction) = transaction {
                            transaction.give_way();
                        }
                        attempts += 1;
                        if attempts < RETRIES {
                            continue;
                        }
                        let error = "other files' transforms kept waiting for the files this one changes with jssgTransform; run the step again".to_string();
                        break indices.iter().map(|_| Err(error.clone())).collect::<Vec<_>>();
                    }
                    let reported = effects.failure.map(|failure| failure.message);
                    let mut outcomes: Vec<Result<FileOutcome, String>> = match outcomes {
                        Ok(outcomes) => outcomes
                            .into_iter()
                            .map(|outcome| {
                                if !outcome.ok {
                                    Err(outcome.error.unwrap_or_else(|| "the callback failed".into()))
                                } else if let Some(message) = reported.as_ref().filter(|_| indices.len() == 1) {
                                    Err(message.clone())
                                } else {
                                    Ok(outcome)
                                }
                            })
                            .collect(),
                        Err(error) => indices.iter().map(|_| Err(error.clone())).collect(),
                    };
                    match (transaction, outcomes.as_mut_slice()) {
                        // One file, in its own transaction.
                        (Some(transaction), [outcome]) => {
                            let keep = match (outcome.as_mut(), compat) {
                                (Err(_), _) => false,
                                (Ok(file), Some(compat)) => {
                                    match keep_changes(target, other_files, file, compat, effects.other_files) {
                                        Ok(keep) => keep,
                                        Err(error) => {
                                            *outcome = Err(error);
                                            false
                                        }
                                    }
                                }
                                (Ok(_), None) if !effects.other_files.is_empty() => {
                                    *outcome = Err(NOT_SUPPORTED.to_string());
                                    false
                                }
                                (Ok(_), None) => true,
                            };
                            transaction.finish(keep);
                        }
                        _ if !effects.other_files.is_empty() => {
                            for outcome in &mut outcomes {
                                if outcome.is_ok() {
                                    *outcome = Err(NOT_SUPPORTED.to_string());
                                }
                            }
                        }
                        _ => {}
                    }
                    break outcomes;
                };
                for (index, outcome) in indices.iter().zip(outcomes) {
                    results.push((*index, outcome));
                    progress();
                }
            }
            Ok(results)
        })
        .await
    })
}

/// Why a callback's `jssgTransform` calls on other files fail.
const NOT_SUPPORTED: &str = "jssgTransform() is not supported in forEachFile callbacks yet; change each file from its own callback";

/// How many times one file's transform may give way to others (see
/// [`Transaction::aborted`]) before it fails.
const RETRIES: usize = 50;

/// What a step's workers share so jssg transforms (run by the YAML
/// translation's `codemod-internal:jssg-compat` adapter) can change other
/// files with `jssgTransform`, as workflow.yaml's `js-ast-grep` step let
/// them: the step's unwritten files, so every change to a file adds up
/// whichever file's transform made it, and the renames among them.
#[derive(Default)]
struct OtherFiles {
    pending: PendingFiles,
    /// By the renamed file's canonical path: where it goes, absolute.
    renames: Mutex<BTreeMap<PathBuf, PathBuf>>,
}

impl OtherFiles {
    /// Every file the step's transforms changed this way, renames included.
    fn into_changes(self) -> Vec<JssgFileChange> {
        let mut renames = self.renames.into_inner().unwrap_or_else(|e| e.into_inner());
        let mut changes = Vec::new();
        for (path, original, content) in self.pending.changed() {
            changes.push(JssgFileChange {
                rename_to: renames.remove(&path),
                path,
                original,
                content,
            });
        }
        // Files only renamed.
        for (path, to) in renames {
            if let Ok(text) = fs::read_to_string(&path) {
                changes.push(JssgFileChange {
                    path,
                    original: text.clone(),
                    content: text,
                    rename_to: Some(to),
                });
            }
        }
        changes
    }
}

/// The callback's file, taken by the adapter's `begin` for the transform.
struct CompatFile {
    taken: TakenFile,
    /// Drop what the transform changed (a step `dry_run`, a shard filter).
    discard: bool,
}

/// The file the worker's current callback took, if it took one.
type CompatSlot = Arc<Mutex<Option<CompatFile>>>;

/// `__jssgCompatHost` for `jssg_compat.ts`: `begin(path, discard)` takes
/// the callback's file from the step's unwritten files and gives its text.
fn install_compat_host<'js>(
    ctx: &Ctx<'js>,
    pending: PendingFiles,
    target: PathBuf,
    slot: CompatSlot,
) -> rquickjs::Result<()> {
    let host = Object::new(ctx.clone())?;
    host.set(
        "begin",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, path: String, discard: bool| -> rquickjs::Result<String> {
                let (text, taken) = pending
                    .take(&target.join(&path))
                    .map_err(|e| Exception::throw_message(&ctx, &e.to_string()))?;
                *slot.lock().unwrap_or_else(|e| e.into_inner()) =
                    Some(CompatFile { taken, discard });
                Ok(text)
            },
        )?,
    )?;
    ctx.globals().set("__jssgCompatHost", host)
}

/// Put what a jssg transform did to its file into the step's unwritten
/// files, with the renames it and its `jssgTransform` calls made. Its
/// effects are then committed from there. Gives whether to keep the
/// transaction's changes.
fn keep_changes(
    target: &Path,
    other_files: &OtherFiles,
    outcome: &mut FileOutcome,
    compat: CompatFile,
    changed: Vec<FileChange>,
) -> Result<bool, String> {
    if compat.discard {
        return Ok(false);
    }
    let mut renames = Vec::new();
    for effect in std::mem::take(&mut outcome.effects) {
        match effect {
            Effect::Write { text } => compat.taken.set(text),
            Effect::Rename { to } => {
                let to = changes::normalize(&to).ok_or_else(|| {
                    format!("cannot rename to '{to}': the path must be relative and stay inside the target")
                })?;
                renames.push((compat.taken.path().to_path_buf(), target.join(to)));
            }
            Effect::Delete => return Err("a jssg transform cannot delete its file".to_string()),
        }
    }
    for change in changed {
        if let ExecutionResult::Modified(ModifiedResult {
            rename_to: Some(to),
            ..
        }) = change.result
        {
            let path = change.path.canonicalize().unwrap_or(change.path);
            renames.push((path, to));
        }
    }
    other_files
        .renames
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .extend(renames);
    Ok(true)
}

/// What a callback's effects leave of one file. `delete()` wins over
/// whatever else the callback did; the last `write()` and `rename()` count.
fn fold(path: String, original: String, effects: Vec<Effect>) -> Result<PlannedChange, String> {
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
        return Ok(PlannedChange {
            path,
            original,
            change: Change::Delete,
        });
    }
    let text = text.unwrap_or_else(|| original.clone());
    Ok(PlannedChange::moved(path, original, text, rename_to))
}
