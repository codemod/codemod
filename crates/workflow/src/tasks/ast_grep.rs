//! `astGrep(path, options)`: apply an ast-grep YAML rule file.

use std::sync::Mutex;

use butterflow_core::execution::CodemodExecutionConfig;
use codemod_sandbox::{scan_file_with_combined_scan, with_combined_scan};
use serde::Deserialize;
use serde_json::json;

use super::changes::{self, bundle_path, relative, PlannedChange};
use super::{Globs, TaskContext, TaskError, TaskOutcome, TaskResult};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Request {
    path: String,
    include: Option<Globs>,
    exclude: Option<Globs>,
    base_path: Option<String>,
    threads: Option<usize>,
}

pub(crate) fn run(task: &TaskContext<'_>, request: Request) -> TaskResult {
    let run = task.run;
    let rules = bundle_path(task, &request.path, "rule file")?;
    if !rules.is_file() {
        return Err(TaskError::Failed(format!(
            "ast-grep rule file not found: {}",
            rules.display()
        )));
    }
    let base_path = changes::base_path(request.base_path.as_deref())?;

    let planned = Mutex::new(Vec::<PlannedChange>::new());
    let failures = Mutex::new(Vec::<(String, String)>::new());
    let matches = std::sync::atomic::AtomicUsize::new(0);
    let unmodified = std::sync::atomic::AtomicUsize::new(0);

    with_combined_scan(&rules.to_string_lossy(), |scan| {
        let languages = scan
            .rule_refs
            .iter()
            .map(|rule| rule.language.to_string())
            .collect::<Vec<_>>();
        let config = CodemodExecutionConfig {
            pre_run_callback: None,
            progress_callback: std::sync::Arc::new(Some(task.walk_progress())),
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
        config.execute(|path, _| {
            if !path.is_file() || run.cancel.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            let display = relative(&run.target, path).unwrap_or_else(|| path.display().to_string());
            let original = std::fs::read_to_string(path).unwrap_or_default();
            match scan_file_with_combined_scan(path, &scan.combined_scan, true) {
                Ok((found, modified, content)) => {
                    matches.fetch_add(found.len(), std::sync::atomic::Ordering::Relaxed);
                    match content.filter(|_| modified) {
                        Some(content) => planned
                            .lock()
                            .unwrap()
                            .push(PlannedChange::edit(display, original, content)),
                        None => {
                            unmodified.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
                Err(error) => failures.lock().unwrap().push((display, error.to_string())),
            }
        })
    })
    .map_err(|e| TaskError::Failed(format!("ast-grep failed: {e}")))?;

    let mut planned = planned.into_inner().unwrap();
    planned.sort_by(|a, b| a.path.cmp(&b.path));
    let failures = failures.into_inner().unwrap();
    let applied = changes::commit(task, &planned, run.dry_run)?;
    let summary = &run.services.summary;
    summary.add_unmodified(unmodified.into_inner());
    summary.add_errors(failures.len());
    if !failures.is_empty() && planned.is_empty() {
        return Err(TaskError::Failed(format!(
            "ast-grep failed on every file; first error: {}: {}",
            failures[0].0, failures[0].1
        )));
    }
    Ok(TaskOutcome::new(json!({
        "changes": applied,
        "matches": matches.into_inner(),
        "failures": failures
            .iter()
            .map(|(path, error)| json!({ "path": path, "error": error }))
            .collect::<Vec<_>>(),
    })))
}
