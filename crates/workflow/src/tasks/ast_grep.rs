//! `astGrep(rules, options)`: apply ast-grep rules. The workflow gives them
//! as JSON, written in the workflow or imported from a YAML file, which the
//! module loader reads as JSON.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use butterflow_core::execution::CodemodExecutionConfig;
use codemod_sandbox::{scan_text_with_combined_scan, with_combined_scan_of};
use serde::Deserialize;
use serde_json::{json, Value};

use super::changes::{self, relative, PlannedChange};
use super::{Globs, TaskContext, TaskError, TaskOutcome, TaskResult};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Request {
    /// The rules, each an object of the shape of an ast-grep YAML rule.
    rules: Vec<Value>,
    include: Option<Globs>,
    exclude: Option<Globs>,
    base_path: Option<String>,
    threads: Option<usize>,
}

pub(crate) fn run(task: &TaskContext<'_>, request: Request) -> TaskResult {
    let run = task.run;
    if request.rules.is_empty() {
        return Err(TaskError::Failed("astGrep needs at least one rule".into()));
    }
    // JSON is YAML: one document per rule, as in a multi-rule file.
    let rules = request
        .rules
        .iter()
        .map(Value::to_string)
        .collect::<Vec<_>>()
        .join("\n---\n");
    let base_path = changes::base_path(request.base_path.as_deref())?;

    let scanned = with_combined_scan_of(&rules, |scan| {
        let languages = scan
            .rule_refs
            .iter()
            .map(|rule| rule.language.to_string())
            .collect::<Vec<_>>();
        let config = CodemodExecutionConfig {
            pre_run_callback: None,
            progress_callback: std::sync::Arc::new(None),
            target_path: Some(run.target.clone()),
            base_path: base_path.clone(),
            include_globs: request.include.clone().map(Globs::into_vec),
            explicit_files: None,
            exclude_globs: request.exclude.clone().map(Globs::into_vec),
            dry_run: true,
            languages: Some(languages),
            threads: request.threads,
            capabilities: None,
        };
        // One walk (`execute` walks twice: once to count, once to visit),
        // then the files go to the threads in walk order.
        let files = config.try_collect_files()?;
        let threads = request
            .threads
            .unwrap_or_else(|| {
                std::thread::available_parallelism()
                    .map_or(1, |n| n.get())
                    .min(12)
            })
            .clamp(1, files.len().max(1));
        let next = AtomicUsize::new(0);
        let processed = AtomicUsize::new(0);
        let total = files.len();
        let every = (total / 100).max(1);
        // (matches, the new text when the fixes changed the file).
        let scan_text = |path: &Path, text: &str| {
            scan_text_with_combined_scan(path, text, &scan.combined_scan, true)
                .map(|(found, modified, content)| (found.len(), content.filter(|_| modified)))
                .map_err(|error| error.to_string())
        };
        let workers: Vec<Scanned> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..threads)
                .map(|_| {
                    let (files, next, processed, scan_text) =
                        (&files, &next, &processed, &scan_text);
                    scope.spawn(move || {
                        let mut out = Scanned::default();
                        loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(path) = files.get(index) else { break };
                            if !task.canceled() {
                                scan_one(task, scan_text, path, &mut out);
                            }
                            let done = processed.fetch_add(1, Ordering::Relaxed) + 1;
                            if done == total || done.is_multiple_of(every) {
                                task.progress(done, total);
                            }
                        }
                        out
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
                })
                .collect()
        });
        Ok(workers)
    })
    .map_err(|e| TaskError::Failed(format!("ast-grep failed: {e}")))?;

    let mut planned = Vec::new();
    let mut failures = Vec::new();
    let mut matches = 0;
    let mut unmodified = 0;
    for worker in scanned {
        planned.extend(worker.planned);
        failures.extend(worker.failures);
        matches += worker.matches;
        unmodified += worker.unmodified;
    }
    planned.sort_by(|a, b| a.path.cmp(&b.path));
    failures.sort();
    let total = planned.len() + unmodified + failures.len();
    let summary = &run.services.summary;
    summary.add_unmodified(unmodified);
    summary.add_errors(failures.len());
    task.log_failures(&failures);
    if !failures.is_empty() && failures.len() == total {
        return Err(TaskError::Failed(format!(
            "ast-grep failed on every file; first error: {}: {}",
            failures[0].0, failures[0].1
        )));
    }
    let applied = changes::commit(task, &planned, run.dry_run)?;
    let failed = failures.len();
    Ok(TaskOutcome::new(json!({
        "changes": applied,
        "matches": matches,
        "failures": failures
            .iter()
            .map(|(path, error)| json!({ "path": path, "error": error }))
            .collect::<Vec<_>>(),
    }))
    .with_failures(failed, total))
}

/// Scan a file's text: its match count, and its new text when the fixes
/// changed it.
type ScanText<'a> = dyn Fn(&Path, &str) -> Result<(usize, Option<String>), String> + Sync + 'a;

/// What one thread's files came to.
#[derive(Default)]
struct Scanned {
    planned: Vec<PlannedChange>,
    failures: Vec<(String, String)>,
    matches: usize,
    unmodified: usize,
}

/// Scan one file the walk found (an absolute path under the target).
fn scan_one(task: &TaskContext<'_>, scan_text: &ScanText<'_>, path: &Path, out: &mut Scanned) {
    let run = task.run;
    // The walker gives regular files (it follows no links) under the target
    // as the run spells it, so the relative path is a prefix away; `relative`
    // (which resolves both paths) is the fallback.
    let display = path
        .strip_prefix(&run.target)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .or_else(|| relative(&run.target, path))
        .unwrap_or_else(|| path.display().to_string());
    // One read: the scan's input is also the original the commit checks.
    let original = match std::fs::read_to_string(path) {
        Ok(text) => text,
        // Gone since the walk: not a file now.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            // As `scan_file_with_combined_scan` reports it.
            out.failures.push((display, format!("IO error: {error}")));
            return;
        }
    };
    match scan_text(path, &original) {
        Ok((found, content)) => {
            out.matches += found;
            match content {
                Some(content) => out
                    .planned
                    .push(PlannedChange::edit(display, original, content)),
                None => out.unmodified += 1,
            }
        }
        Err(error) => out.failures.push((display, error)),
    }
}
