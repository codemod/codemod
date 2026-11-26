//! `forEachFile(selector, astGrep(rules))`: apply ast-grep rules to the
//! selected files, in Rust, with no JavaScript per file. The workflow gives
//! the rules as JSON, written in the workflow or imported from a YAML file,
//! which the module loader reads as JSON.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

use butterflow_core::execution::{language_globs, CodemodExecutionConfig};
use codemod_sandbox::sandbox::engine::language_data::get_extensions_for_language;
use codemod_sandbox::{scan_pass_as, with_combined_scan_of, LanguageScan};
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
    /// Only files whose text contains this string.
    containing: Option<String>,
    /// Passes over each file at most: the rules run again on the text the
    /// last pass fixed, until it no longer changes. One without.
    repeat: Option<usize>,
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

    let scanned = with_combined_scan_of(&rules, |scan| {
        let languages = scan
            .rule_refs
            .iter()
            .map(|rule| rule.language.to_string())
            .collect::<Vec<_>>();
        // Each language's rules run on the files with its extensions, parsed
        // as that language: a `typescript` rule selects `.js` files and runs
        // on them as TypeScript, as `forEachFile({ language: "typescript" })`
        // callbacks parse them. Files the include globs pick that no rule is
        // for (another language, or none) are skipped, as the default
        // selection (the rules' languages) would not have them.
        let scans = scan
            .by_language()
            .into_iter()
            .map(|scan| {
                let extensions = get_extensions_for_language(scan.language)
                    .into_iter()
                    .map(str::to_ascii_lowercase)
                    .collect();
                (scan, extensions)
            })
            .collect::<Vec<_>>();
        let filter = Filter {
            scans: &scans,
            containing: request.containing.as_deref(),
        };
        let config = CodemodExecutionConfig {
            pre_run_callback: None,
            progress_callback: std::sync::Arc::new(None),
            target_path: Some(run.target.clone()),
            base_path: None,
            include_globs: request.include.clone().map(Globs::into_vec),
            explicit_files: None,
            exclude_globs: request.exclude.clone().map(Globs::into_vec),
            dry_run: true,
            languages: Some(languages),
            threads: None,
            capabilities: None,
        };
        // One walk (`execute` walks twice: once to count, once to visit),
        // then the files go to the threads in walk order.
        let include = match &request.include {
            Some(globs) => globs.clone().into_vec(),
            None => {
                let mut globs = Vec::new();
                for language in config.languages.iter().flatten() {
                    globs.extend(language_globs(language)?);
                }
                globs
            }
        };
        let exclude = request.exclude.clone().map(Globs::into_vec).unwrap_or_default();
        let snapshot = run
            .overlay
            .as_ref()
            .and_then(|overlay| overlay.select_snapshot(&include, &exclude));
        let mut files = match snapshot {
            Some(paths) => paths?
                .into_iter()
                .map(|path| changes::under(&run.target, &path))
                .collect(),
            None => config.try_collect_files()?,
        };
        // In a dry run, the files earlier steps would have removed are gone,
        // and the ones they would have renamed into place are there.
        if let Some(overlay) = &run.overlay {
            files.retain(|file| match file.strip_prefix(&run.target) {
                Ok(path) => !overlay.is_gone(&path.to_string_lossy().replace('\\', "/")),
                Err(_) => true,
            });
            for path in overlay.created(&include, &exclude)? {
                files.push(changes::under(&run.target, &path));
            }
        }
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .min(12)
            .clamp(1, files.len().max(1));
        let next = AtomicUsize::new(0);
        let processed = AtomicUsize::new(0);
        let total = files.len();
        let every = (total / 100).max(1);
        let passes = request.repeat.unwrap_or(1).max(1);
        // One pass of every language's rules that covers the file, one
        // language after another on the text the last one fixed.
        let pass = |scans: &[&LanguageScan<'_>], path: &Path, text: &str| {
            let mut fixed: Option<String> = None;
            let (mut matches, mut fixes) = (0, 0);
            for scan in scans {
                let input = fixed.as_deref().unwrap_or(text);
                let pass = scan_pass_as(scan.language, path, input, &scan.combined_scan)
                    .map_err(|error| error.to_string())?;
                matches += pass.matches;
                fixes += pass.fixes;
                if let Some(next) = pass.content {
                    fixed = Some(next);
                }
            }
            // One language's rules can undo another's: the text is back
            // where it started though rules changed it.
            if fixed.as_deref() == Some(text) && scans.len() > 1 {
                return Err("rules of different languages undo each other's fixes on this file, so it was left as it was".to_string());
            }
            Ok::<_, String>((matches, fixes, fixed.filter(|fixed| fixed != text)))
        };
        // (matches, the new text when the fixes changed the file).
        let scan_text = |scans: &[&LanguageScan<'_>],
                         path: &Path,
                         text: &str|
         -> Result<(usize, Option<String>), String> {
            if passes == 1 {
                return pass(scans, path, text).map(|(matches, _, content)| (matches, content));
            }
            // Repeat: run the rules on the text the last pass fixed, in
            // memory, so a dry run ends with the same text. Of overlapping
            // matches a pass fixes the outer one; the next finds the inner.
            // A pass that changes the text counts the fixes it applied, the
            // last one the matches left (of rules without a fix).
            let mut fixed: Option<String> = None;
            let mut matches = 0;
            for _ in 0..passes {
                let input = fixed.as_deref().unwrap_or(text);
                match pass(scans, path, input)? {
                    (found, _, None) => return Ok((matches + found, fixed)),
                    (_, fixes, Some(next)) => {
                        matches += fixes;
                        fixed = Some(next);
                    }
                }
            }
            // Every pass changed the text: one more says whether it is done.
            let input = fixed.as_deref().unwrap_or(text);
            let (found, _, last) = pass(scans, path, input)?;
            if last.is_some() {
                return Err(format!(
                    "the rules still change this file after {passes} passes, so it was left as it was; a fix may match its own output (give repeat a higher number, or change the rule)"
                ));
            }
            Ok((matches + found, fixed))
        };
        let workers: Vec<Scanned> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..threads)
                .map(|_| {
                    let (files, next, processed, scan_text, filter) =
                        (&files, &next, &processed, &scan_text, &filter);
                    scope.spawn(move || {
                        let mut out = Scanned::default();
                        loop {
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(path) = files.get(index) else { break };
                            if !task.canceled() {
                                scan_one(task, scan_text, filter, path, &mut out);
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

/// Scan a file's text with the rules of the languages that cover it: its
/// match count, and its new text when the fixes changed it.
type ScanText<'a> = dyn Fn(&[&LanguageScan<'_>], &Path, &str) -> Result<(usize, Option<String>), String>
    + Sync
    + 'a;

/// What one thread's files came to.
#[derive(Default)]
struct Scanned {
    planned: Vec<PlannedChange>,
    failures: Vec<(String, String)>,
    matches: usize,
    unmodified: usize,
}

/// Which of the walked files are scanned, and with which rules.
struct Filter<'a, 'r> {
    /// Each language's rules, with its extensions (`.ts`, ...), lowercase.
    scans: &'a [(LanguageScan<'r>, Vec<String>)],
    containing: Option<&'a str>,
}

impl<'r> Filter<'_, 'r> {
    /// The rules of the languages whose extensions the file has.
    fn covering(&self, path: &Path) -> Vec<&LanguageScan<'r>> {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        self.scans
            .iter()
            .filter(|(_, extensions)| {
                extensions
                    .iter()
                    .any(|extension| name.ends_with(extension.as_str()))
            })
            .map(|(scan, _)| scan)
            .collect()
    }
}

/// Scan one file the walk found (an absolute path under the target).
fn scan_one(
    task: &TaskContext<'_>,
    scan_text: &ScanText<'_>,
    filter: &Filter<'_, '_>,
    path: &Path,
    out: &mut Scanned,
) {
    let scans = filter.covering(path);
    if scans.is_empty() {
        return;
    }
    let run = task.run;
    // The walker gives regular files (it follows no links) under the target
    // as the run spells it, so the relative path is a prefix away; `relative`
    // (which resolves both paths) is the fallback.
    let display = path
        .strip_prefix(&run.target)
        .ok()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .or_else(|| relative(&run.target, path));
    let Some(display) = display else {
        out.failures.push((
            path.display().to_string(),
            "the scan path must stay inside the target".into(),
        ));
        return;
    };
    // One read: the scan's input is also the original the commit checks.
    // In a dry run, the text earlier steps would have left.
    let read = match &run.overlay {
        Some(overlay) => overlay.read(&display, path),
        None => std::fs::read_to_string(path),
    };
    let original = match read {
        Ok(text) => text,
        // Gone since the walk: not a file now.
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            // As `scan_file_with_combined_scan` reports it.
            out.failures.push((display, format!("IO error: {error}")));
            return;
        }
    };
    if filter
        .containing
        .is_some_and(|needle| !original.contains(needle))
    {
        return;
    }
    match scan_text(&scans, path, &original) {
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
