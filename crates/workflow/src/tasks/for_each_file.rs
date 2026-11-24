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

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use butterflow_core::execution::{self, language_globs};
use butterflow_core::jssg_runner::{JssgFileChange, SemanticMode};
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
use super::overlay::Overlay;
use super::reduce::{self, FinishOnDrop, Fold, InOrder, Reduce};
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
pub(crate) struct FileOutcome {
    pub ok: bool,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub effects: Vec<Effect>,
    #[serde(default)]
    pub output: Value,
    #[serde(default)]
    pub has_output: bool,
}

#[derive(Deserialize)]
struct Request {
    #[serde(default)]
    selector: Selector,
    callback: String,
    captures: Value,
    /// `forEachFile(selector, callback, reducer, initial)`: fold the
    /// outputs into one value instead of listing them.
    #[serde(default)]
    reduce: Option<Reduce>,
}

pub(crate) fn run(task: &TaskContext<'_>, request: Value) -> Result<TaskOutcome, String> {
    let request: Request =
        serde_json::from_value(request).map_err(|e| format!("bad forEachFile task: {e}"))?;
    let run = task.run;
    let (selector, files) = select_paths(run, request.selector)?;
    let Ran {
        outcomes,
        other_changes,
        reduced,
    } = run_callback(
        task,
        &request.callback,
        &request.captures,
        &selector,
        &files,
        request.reduce.as_ref(),
    )?;

    let mut total = 0;
    let mut outputs = Vec::new();
    let mut failures: Vec<(String, String)> = Vec::new();
    let mut planned = Vec::new();
    for (Selected { path, .. }, outcome) in files.into_iter().zip(outcomes) {
        let outcome = outcome.unwrap_or_else(|| Err("the file was not processed".to_string()));
        if matches!(outcome, Ok(Done::Skipped)) {
            continue;
        }
        total += 1;
        match outcome {
            Ok(Done::Skipped) => {}
            Err(error) => failures.push((path, error)),
            Ok(Done::Ran { outcome, original }) => {
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
    // A reducer that failed fails the step before anything is written; the
    // files left when it stopped are not failures of their own.
    let reduced = reduced.transpose()?;
    task.log_failures(&failures);
    every_file_failed(&failures, total)?;
    planned.extend(changes::plan(task, &other_changes)?);
    let changes = changes::commit(task, &planned, run.dry_run)?;
    let failed = failures.len();
    let failures: Vec<Value> = failures
        .into_iter()
        .map(|(path, error)| json!({ "path": path, "error": error }))
        .collect();
    let result = match reduced {
        Some(reduced) => json!({
            "reduced": reduced,
            "changes": changes,
            "failures": failures,
        }),
        None => json!({
            "outputs": outputs,
            "changes": changes,
            "failures": failures,
        }),
    };
    Ok(TaskOutcome::new(result).with_failures(failed, total))
}

/// What became of one selected file.
enum Done {
    /// Not UTF-8, or without the selector's `containing` text: not one of
    /// the step's files.
    Skipped,
    /// The callback ran. `original` is the text it was given, kept only when
    /// it asked for changes. With a reducer, its output went to the fold.
    Ran {
        outcome: FileOutcome,
        original: Option<String>,
    },
}

/// What a step's callbacks did.
struct Ran {
    /// What became of each of the step's files, in their order (`None` for
    /// a file the callback did not get to).
    outcomes: Vec<Option<Result<Done, String>>>,
    /// The files jssg transforms changed through the step's unwritten files
    /// (see [`OtherFiles`]).
    other_changes: Vec<JssgFileChange>,
    /// With a reducer: the folded outputs, or why the fold failed.
    reduced: Option<Result<Value, String>>,
}

/// Run the extracted callback `callback` on the selected `files`, in
/// workers, as the selector says (language, concurrency, semantic analysis,
/// capabilities, `containing`). Each worker reads its files. Gives each
/// file's outcome, in the order of `files`; nothing is committed.
fn run_callback(
    task: &TaskContext<'_>,
    callback: &str,
    captures: &Value,
    selector: &Selector,
    files: &[Selected],
    reduce: Option<&Reduce>,
) -> Result<Ran, String> {
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
            .map(|selected| {
                selected
                    .file
                    .canonicalize()
                    .unwrap_or_else(|_| selected.file.clone())
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
    let mut outcomes: Vec<Option<Result<Done, String>>> = (0..files.len()).map(|_| None).collect();
    let other_files = OtherFiles {
        pending: run
            .overlay
            .as_ref()
            .map(Overlay::pending_files)
            .unwrap_or_default(),
        renames: Mutex::default(),
    };

    // With a reducer, outputs are folded in their own isolate as files
    // finish, in the files' order; a slow reducer holds the workers back
    // rather than letting outputs pile up.
    let fold = reduce
        .map(|reduce| Fold::new(task, reduce, &isolates))
        .transpose()?;
    let (sender, entries) = std::sync::mpsc::sync_channel::<reduce::Entry>(64);
    let in_order = fold
        .is_some()
        .then(|| InOrder::new(sender, reduce::window(workers, concurrency)));
    let in_order = in_order.as_ref();

    let (worker_results, reduced): (Vec<WorkerResult>, _) = std::thread::scope(|scope| {
        let cancel = &*task.cancel;
        let folding = fold.map(|fold| scope.spawn(move || fold.run(entries, cancel)));
        let worker_results = if files.is_empty() {
            Vec::new()
        } else {
            let handles: Vec<_> = (0..workers)
                .map(|_| {
                    let mut options = isolates.worker_options(task, selector.language.clone());
                    // Only workers that run one file at a time keep what a
                    // file's transforms change in a transaction of its own;
                    // other workers' `jssgTransform` changes are refused, and
                    // must not reach the step's.
                    if concurrency == 1 {
                        options.pending_files = Some(other_files.pending.clone());
                    }
                    let job = Job {
                        options,
                        registry: isolates.registry.clone(),
                        name: &name,
                        module,
                        captures: &captures,
                        language: selector.language.as_deref(),
                        files,
                        containing: selector.containing.as_deref(),
                        next: &next,
                        concurrency,
                        cancel: &task.cancel,
                        workflow_path: &run.workflow_path,
                        target: &run.target,
                        other_files: &other_files,
                        overlay: run.overlay.as_deref(),
                        in_order,
                    };
                    let progress = &progress;
                    scope.spawn(move || {
                        let stop = FinishOnDrop(in_order);
                        let result = worker(job, progress);
                        if result.is_ok() {
                            stop.disarm();
                        }
                        result
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
        };
        // Every file is done: the fold ends with what it was given.
        if let Some(in_order) = in_order {
            in_order.finish();
        }
        let reduced = folding.map(|folding| {
            folding
                .join()
                .unwrap_or_else(|_| Err("the reducer panicked".to_string()))
        });
        (worker_results, reduced)
    });
    for result in worker_results {
        for (index, outcome) in result? {
            outcomes[index] = Some(outcome);
        }
    }
    Ok(Ran {
        outcomes,
        other_changes: other_files.into_changes(run.overlay.as_deref()),
        reduced,
    })
}

/// A file's path, and what the callback returned for it or why it failed.
pub(crate) type FileOutput = (String, Result<Value, String>);

/// What the callback returned for each of the selected `files` (`null` for
/// nothing), without committing what it did to them; files it failed on are
/// errors, files that are not UTF-8 are left out. For `shard`'s filter.
pub(crate) fn callback_outputs(
    task: &TaskContext<'_>,
    callback: &str,
    captures: &Value,
    selector: &Selector,
    files: &[Selected],
) -> Result<Vec<FileOutput>, String> {
    // A filter changes nothing: what transforms changed is dropped.
    let Ran { outcomes, .. } = run_callback(task, callback, captures, selector, files, None)?;
    Ok(files
        .iter()
        .zip(outcomes)
        .filter_map(|(Selected { path, .. }, outcome)| {
            let output =
                match outcome.unwrap_or_else(|| Err("the file was not processed".to_string())) {
                    Ok(Done::Skipped) => return None,
                    Ok(Done::Ran { outcome, .. }) => match outcome.error {
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
    let (selector, selected) = select_paths(run, selector)?;
    let files = read_selected(
        selected,
        selector.containing.as_deref(),
        run.overlay.as_deref(),
    );
    Ok((selector, files))
}

/// The files the globs of a selector pick in the run's target, unread, with
/// the selector as completed from the package's languages when it names
/// none. Its `containing` filter is left for when the files are read.
pub(crate) fn select_paths(
    run: &RunContext,
    mut selector: Selector,
) -> Result<(Selector, Vec<Selected>), String> {
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
    let mut paths = select(&run.target, &selector, &default_languages)?;
    // In a dry run, the files earlier steps would have removed are gone, and
    // the ones they would have renamed into place are there.
    if let Some(overlay) = &run.overlay {
        paths.retain(|selected| !overlay.is_gone(&selected.path));
        let include = include_globs(&selector, &default_languages)?;
        for path in overlay.created(&include, &selector.exclude)? {
            paths.push(Selected {
                file: changes::under(&run.target, &path),
                path,
            });
        }
        paths.sort_by(|a, b| a.path.cmp(&b.path));
    }
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

/// A selected file, not read yet.
#[derive(Debug, Clone)]
pub(crate) struct Selected {
    /// Relative to the target, with `/`: what the callback gets.
    pub path: String,
    /// The file as the walk found it, to read it by: `path` may not lead
    /// back to it (a `\` in a name on Unix, a name that is not UTF-8).
    pub file: PathBuf,
}

/// Select files by globs and `.gitignore` (as `astGrep` selects its),
/// without reading them. Sorted by path. The selector's text filter is
/// [`read_selected`]'s.
pub(crate) fn select(
    target: &Path,
    selector: &Selector,
    default_languages: &[String],
) -> Result<Vec<Selected>, String> {
    let include = include_globs(selector, default_languages)?;
    execution::select_files(target, &include, &selector.exclude)?
        .into_iter()
        .map(|file| {
            let path = file
                .strip_prefix(target)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            Ok(Selected { path, file })
        })
        .collect()
}

/// The globs a selector picks files by: its own, or its language's files.
fn include_globs(selector: &Selector, default_languages: &[String]) -> Result<Vec<String>, String> {
    if !selector.include.is_empty() {
        return Ok(selector.include.clone());
    }
    let languages: Vec<&str> = match selector.language.as_deref() {
        Some(language) => vec![language],
        None => default_languages.iter().map(String::as_str).collect(),
    };
    let mut globs = Vec::new();
    for language in languages {
        globs.extend(language_globs(language)?);
    }
    Ok(globs)
}

/// The text of a selected file, or `None` when it is not one of the step's
/// files: unreadable, not UTF-8, or without the `containing` text. In a dry
/// run, the text earlier steps would have left.
fn read_selected_file(
    selected: &Selected,
    containing: Option<&str>,
    overlay: Option<&Overlay>,
) -> Option<String> {
    let text = match overlay {
        Some(overlay) => overlay.read(&selected.path, &selected.file).ok()?,
        None => fs::read_to_string(&selected.file).ok()?,
    };
    match containing {
        Some(needle) if !text.contains(needle) => None,
        _ => Some(text),
    }
}

/// The selected files with their text (by their paths relative to the
/// target), read in parallel; those [`read_selected_file`] leaves out are
/// dropped.
pub(crate) fn read_selected(
    selected: Vec<Selected>,
    containing: Option<&str>,
    overlay: Option<&Overlay>,
) -> Vec<(String, String)> {
    let texts = parallel_map(&selected, |selected| {
        read_selected_file(selected, containing, overlay)
    });
    selected
        .into_iter()
        .zip(texts)
        .filter_map(|(selected, text)| Some((selected.path, text?)))
        .collect()
}

type WorkerResult = Result<Vec<(usize, Result<Done, String>)>, String>;

struct Job<'a> {
    options: WorkerOptions<OxcResolver>,
    registry: Option<Arc<dyn Registry>>,
    name: &'a str,
    module: &'a str,
    captures: &'a str,
    language: Option<&'a str>,
    files: &'a [Selected],
    containing: Option<&'a str>,
    next: &'a AtomicUsize,
    concurrency: usize,
    cancel: &'a Arc<AtomicBool>,
    workflow_path: &'a Path,
    target: &'a Path,
    other_files: &'a OtherFiles,
    overlay: Option<&'a Overlay>,
    /// With a reducer: where each file's output goes.
    in_order: Option<&'a InOrder>,
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
        containing,
        next,
        concurrency,
        cancel,
        workflow_path,
        target,
        other_files,
        overlay,
        in_order,
    } = job;
    block_on(async move {
        let worker = isolate::start(options, registry).await?;
        let sandbox = &worker;
        let results = async_with!(sandbox.context => |ctx| {
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
                install_compat_host(&ctx, other_files.pending.clone(), Arc::clone(&slot))
                    .map_err(|e| format!("could not start the step's runtime: {e}"))?;
            }

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
                // Not too far ahead of the reducer.
                if in_order.is_some_and(|in_order| !in_order.wait_turn(start, cancel)) {
                    break;
                }
                // Read this batch's files; the ones that are not the step's
                // are done.
                let mut indices = Vec::with_capacity(range.len());
                let mut texts = Vec::with_capacity(range.len());
                for index in range {
                    match read_selected_file(&files[index], containing, overlay) {
                        Some(text) => {
                            indices.push(index);
                            texts.push(text);
                        }
                        None => {
                            if let Some(in_order) = in_order {
                                in_order.put(index, None);
                            }
                            results.push((index, Ok(Done::Skipped)));
                            progress();
                        }
                    }
                }
                if indices.is_empty() {
                    continue;
                }
                let mut attempts = 0;
                let outcomes = loop {
                    let transaction = (concurrency == 1).then(|| other_files.pending.begin());
                    if transaction.is_some() {
                        // The one file the adapter may take for this callback.
                        let selected = &files[indices[0]];
                        slot.lock().unwrap_or_else(|e| e.into_inner()).current =
                            Some((selected.path.clone(), selected.file.clone()));
                    }
                    let settled = async {
                        let paths: Vec<&str> = indices.iter().map(|&index| files[index].path.as_str()).collect();
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
                    let compat = {
                        let mut slot = slot.lock().unwrap_or_else(|e| e.into_inner());
                        slot.current = None;
                        slot.taken.take()
                    };
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
                            let (keep, renames) = match (outcome.as_mut(), compat) {
                                (Err(_), _) => (false, Vec::new()),
                                (Ok(file), Some(compat)) => {
                                    match keep_changes(target, file, compat, effects.other_files) {
                                        Ok(Some(renames)) => (true, renames),
                                        Ok(None) => (false, Vec::new()),
                                        Err(error) => {
                                            *outcome = Err(error);
                                            (false, Vec::new())
                                        }
                                    }
                                }
                                (Ok(_), None) if !effects.other_files.is_empty() => {
                                    *outcome = Err(NOT_SUPPORTED.to_string());
                                    (false, Vec::new())
                                }
                                (Ok(_), None) => (true, Vec::new()),
                            };
                            // A transform still running would change files after
                            // they are given back: refuse what the callback did.
                            let keep = if keep && transaction.unsettled() {
                                *outcome = Err(UNSETTLED.to_string());
                                false
                            } else {
                                keep
                            };
                            // Its renames count only when its changes do.
                            if keep {
                                other_files
                                    .renames
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner())
                                    .extend(renames);
                            }
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
                for ((index, outcome), text) in indices.iter().zip(outcomes).zip(texts) {
                    // The text is only needed to commit changes: a jssg
                    // transform's are in the step's unwritten files already.
                    let outcome = outcome.map(|mut outcome| {
                        // With a reducer the output goes to the fold, not
                        // into the step's result.
                        if let Some(in_order) = in_order {
                            let entry = outcome.has_output.then(|| {
                                let output = std::mem::take(&mut outcome.output);
                                (files[*index].path.clone(), output.to_string())
                            });
                            outcome.has_output = false;
                            in_order.put(*index, entry);
                        }
                        let original = (!outcome.effects.is_empty()).then_some(text);
                        Done::Ran { outcome, original }
                    });
                    if let (Some(in_order), Err(_)) = (in_order, &outcome) {
                        in_order.put(*index, None);
                    }
                    results.push((*index, outcome));
                    progress();
                }
            }
            Ok(results)
        })
        .await;
        worker.shutdown(isolate::SHUTDOWN_GRACE).await;
        results
    })
}

/// Why a callback's `jssgTransform` calls on other files fail.
const NOT_SUPPORTED: &str = "jssgTransform() is not supported in forEachFile callbacks yet; change each file from its own callback";

/// Why a callback that left a `jssgTransform` call running fails.
const UNSETTLED: &str =
    "a jssgTransform() call was still running when the transform finished; await it";

/// How many times one file's transform may give way to others (see
/// [`Transaction::aborted`]) before it fails.
const RETRIES: usize = 50;

/// What a step's workers share so jssg transforms (run by the YAML
/// translation's `codemod-internal:jssg-compat` adapter) can change other
/// files with `jssgTransform`, as workflow.yaml's `js-ast-grep` step let
/// them: the step's unwritten files, so every change to a file adds up
/// whichever file's transform made it, and the renames among them.
struct OtherFiles {
    pending: PendingFiles,
    /// By the renamed file's canonical path: where it goes, absolute.
    renames: Mutex<BTreeMap<PathBuf, PathBuf>>,
}

impl OtherFiles {
    /// Every file the step's transforms changed this way, renames included.
    fn into_changes(self, overlay: Option<&Overlay>) -> Vec<JssgFileChange> {
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
            let text = match overlay {
                Some(overlay) => overlay.read_file(&path),
                None => fs::read_to_string(&path),
            };
            if let Ok(text) = text {
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

/// The worker's current callback: the file it runs on (its path as the
/// callback has it, and as the walk found it), and that file once the
/// adapter took it.
#[derive(Default)]
struct CompatState {
    current: Option<(String, PathBuf)>,
    taken: Option<CompatFile>,
}

type CompatSlot = Arc<Mutex<CompatState>>;

/// `__jssgCompatHost` for `jssg_compat.ts`: `begin(path, discard)` takes
/// the callback's own file from the step's unwritten files and gives its
/// text. Any other path is refused: the file is one the step selected, so
/// what is read and written through it stays the step's own file.
fn install_compat_host<'js>(
    ctx: &Ctx<'js>,
    pending: PendingFiles,
    slot: CompatSlot,
) -> rquickjs::Result<()> {
    let host = Object::new(ctx.clone())?;
    host.set(
        "begin",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, path: String, discard: bool| -> rquickjs::Result<String> {
                let mut state = slot.lock().unwrap_or_else(|e| e.into_inner());
                let file = match &state.current {
                    Some((current, file)) if *current == path => file.clone(),
                    _ => {
                        return Err(Exception::throw_message(
                            &ctx,
                            &format!("'{path}' is not the file this callback runs on"),
                        ))
                    }
                };
                if state.taken.is_some() {
                    return Err(Exception::throw_message(
                        &ctx,
                        &format!("'{path}' is being transformed already"),
                    ));
                }
                let (text, taken) = pending
                    .take(&file)
                    .map_err(|e| Exception::throw_message(&ctx, &e.to_string()))?;
                state.taken = Some(CompatFile { taken, discard });
                Ok(text)
            },
        )?,
    )?;
    ctx.globals().set("__jssgCompatHost", host)
}

/// Put what a jssg transform did to its file into the step's unwritten
/// files, with the renames it and its `jssgTransform` calls made. Its
/// effects are then committed from there. Gives the renames to keep, or
/// `None` when the transaction's changes are dropped.
fn keep_changes(
    target: &Path,
    outcome: &mut FileOutcome,
    compat: CompatFile,
    changed: Vec<FileChange>,
) -> Result<Option<Vec<(PathBuf, PathBuf)>>, String> {
    if compat.discard {
        return Ok(None);
    }
    let mut renames = Vec::new();
    for effect in std::mem::take(&mut outcome.effects) {
        match effect {
            Effect::Write { text } => {
                if !compat.taken.set(text) {
                    return Err(UNSETTLED.to_string());
                }
            }
            Effect::Rename { to } => {
                let to = changes::normalize(&to).ok_or_else(|| {
                    format!("cannot rename to '{to}': the path must be relative and stay inside the target")
                })?;
                renames.push((
                    compat.taken.path().to_path_buf(),
                    changes::under(target, &to),
                ));
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
    Ok(Some(renames))
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
    let text = text.unwrap_or_else(|| original.clone());
    let plan = PlannedChange::moved(path, original, text, rename_to);
    Ok(match &plan.change {
        Change::Edit(text) if *text == plan.original => None,
        _ => Some(plan),
    })
}
