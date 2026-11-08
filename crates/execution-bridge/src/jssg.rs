//! One JSSG batch: the transform bundle (verified against its hash and
//! loaded from memory under a virtual module name), the language, the
//! optional static selector, the invocation input, and one optional semantic
//! provider are set up once; every supplied file is then transformed serially
//! from the content the host sent (snapshot semantics: no transform sees
//! another's edits, and the workspace index is built once from the whole
//! supplied set before the first transform). Files the selector does not
//! match are skipped before any JavaScript runtime starts. Edits, renames,
//! and JSON outputs come back as data with every path validated against the
//! canonical target root. Nothing here reads an author file or a repository
//! listing, and nothing writes repository files.
//!
//! Every batch rule (planning, containment, selector skip, result
//! conversion) is `crates/jssg-batch`, shared with the in-process runtimes
//! (`node` through `crates/jssg-napi`, `quickjs` through
//! `crates/orchestration-quickjs-host`); this module only runs each file in a
//! fresh sandbox runtime and adds semantic analysis.

use std::{path::Path, sync::Arc};

use codemod_sandbox::sandbox::{
    engine::{execute_codemod_with_loader, JssgExecutionOptions},
    resolvers::{InMemoryLoader, InMemoryResolver},
};

pub use jssg_batch::{validate_relative_path, Batch, BatchPlan, Edit, FileOutcome, SemanticScope};

pub async fn transform_batch(batch: Batch<'_>) -> Result<Vec<FileOutcome>, String> {
    use bench_timings::Phase;
    // Benchmark timings: planning is `plan`; the sandbox records setup,
    // parse, transform, and convert per file; the selector check is `parse`
    // and `BatchPlan::finish` is `convert`.
    let plan = bench_timings::span(Phase::Plan, || BatchPlan::new(&batch))?;
    let provider = plan.provider.clone();
    let source = batch.source.unwrap_or_default();
    let module = plan.module.clone();
    let mut resolver = InMemoryResolver::new();
    resolver.add_module_with_source(format!("./{module}"), module.clone(), source.to_string());
    let resolver = Arc::new(resolver);

    // The whole selected set is indexed, including files the selector will
    // skip, so a matching transform can resolve definitions in them.
    for (index, file) in batch.files.iter().enumerate() {
        plan.index(index, &file.content)?;
    }

    let mut outcomes = Vec::with_capacity(batch.files.len());
    for (index, (file, source)) in batch.files.iter().zip(&plan.sources).enumerate() {
        if bench_timings::span(Phase::Parse, || plan.skips(&file.content)) {
            outcomes.push(FileOutcome::skipped(&file.path));
            continue;
        }
        let output = execute_codemod_with_loader(
            JssgExecutionOptions {
                script_path: Path::new(&module),
                resolver: Arc::clone(&resolver),
                language: plan.language,
                file_path: source,
                content: &file.content,
                selector_config: None,
                params: plan.params.clone(),
                matrix_values: None,
                capabilities: None,
                semantic_provider: provider.clone(),
                metrics_context: None,
                llm_request_handler: None,
                shared_state_context: None,
                runtime_event_callback: None,
                cancellation_flag: None,
                test_mode: false,
                dry_run: false,
                stage_writes: true,
                target_directory: &plan.root,
            },
            InMemoryLoader::new(Arc::clone(&resolver)),
        )
        .await
        .map_err(|error| error.to_string());
        outcomes.push(bench_timings::span(Phase::Convert, || {
            plan.finish(index, output)
        })?);
    }
    Ok(outcomes)
}
