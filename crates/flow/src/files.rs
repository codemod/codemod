//! `forEachFile` executor: selects files, runs the extracted callback on a
//! pool of threads (one QuickJS context per thread), and commits the
//! recorded file effects in one go.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use codemod_sandbox::AstGrepModule;
use ignore::overrides::OverrideBuilder;
use ignore::WalkBuilder;
use rquickjs::loader::{BuiltinLoader, BuiltinResolver, ModuleLoader};
use rquickjs::{CatchResultExt, Context, Function, Module, Runtime};
use serde::Deserialize;
use serde_json::{json, Value};

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

/// What one file ends up as after its effects are folded together.
struct Planned {
    path: String,
    original: String,
    text: String,
    rename_to: Option<String>,
    delete: bool,
}

pub struct ForEachFile<'a> {
    pub target: &'a Path,
    pub module: &'a str,
    pub selector: &'a Selector,
    pub captures: &'a str,
    pub dry_run: bool,
    /// Serializes commits from steps running at the same time.
    pub commit_lock: &'a Mutex<()>,
}

pub fn run(task: ForEachFile<'_>) -> Result<Value, String> {
    let files = select(task.target, task.selector)?;

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
                        task.module,
                        task.captures,
                        task.selector.language.as_deref(),
                        &files,
                        &next,
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

    let changes = commit(task.target, &planned, task.dry_run, task.commit_lock)?;
    Ok(json!({ "outputs": outputs, "changes": changes, "failures": failures }))
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

fn worker(
    module: &str,
    captures: &str,
    language: Option<&str>,
    files: &[(String, String)],
    next: &AtomicUsize,
) -> WorkerResult {
    let runtime = Runtime::new().map_err(|e| e.to_string())?;
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
        }
        Ok(results)
    })
}

fn fold(path: String, original: String, effects: Vec<Effect>) -> Result<Planned, String> {
    let mut plan = Planned {
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

/// Check the plan for conflicts, then apply it unless this is a dry run.
/// Either every change is applied or none is.
fn commit(
    target: &Path,
    planned: &[Planned],
    dry_run: bool,
    lock: &Mutex<()>,
) -> Result<Value, String> {
    let vacated: HashSet<&str> = planned
        .iter()
        .filter(|p| p.delete || p.rename_to.is_some())
        .map(|p| p.path.as_str())
        .collect();
    let mut claimed: HashMap<&str, &str> = HashMap::new();
    for plan in planned.iter().filter(|p| !p.delete) {
        let Some(to) = plan.rename_to.as_deref() else {
            continue;
        };
        if let Some(other) = claimed.insert(to, plan.path.as_str()) {
            return Err(format!(
                "conflicting changes: '{other}' and '{}' are both renamed to '{to}'",
                plan.path
            ));
        }
        let kept_in_place = planned
            .iter()
            .any(|p| p.path == to && !vacated.contains(p.path.as_str()));
        if kept_in_place || (!vacated.contains(to) && target.join(to).exists()) {
            return Err(format!(
                "conflicting changes: '{}' is renamed to '{to}', which already exists",
                plan.path
            ));
        }
    }

    let mut written = Vec::new();
    let mut renamed = Vec::new();
    let mut deleted = Vec::new();
    let mut removals: Vec<PathBuf> = Vec::new();
    let mut writes: BTreeMap<String, &str> = BTreeMap::new();
    for plan in planned {
        if plan.delete {
            deleted.push(plan.path.clone());
            removals.push(target.join(&plan.path));
            continue;
        }
        let changed = plan.text != plan.original;
        match &plan.rename_to {
            Some(to) => {
                renamed.push(json!({ "from": plan.path, "to": to }));
                removals.push(target.join(&plan.path));
                writes.insert(to.clone(), &plan.text);
                if changed {
                    written.push(to.clone());
                }
            }
            None if changed => {
                written.push(plan.path.clone());
                writes.insert(plan.path.clone(), &plan.text);
            }
            None => {}
        }
    }

    if !dry_run {
        let _guard = lock
            .lock()
            .map_err(|_| "commit lock poisoned".to_string())?;
        for path in &removals {
            fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        for (path, text) in &writes {
            let full = target.join(path);
            if let Some(parent) = full.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            fs::write(&full, text).map_err(|e| format!("{}: {e}", full.display()))?;
        }
    }

    Ok(json!({ "written": written, "renamed": renamed, "deleted": deleted }))
}

fn is_safe_relative(path: &str) -> bool {
    let path = Path::new(path);
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
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
