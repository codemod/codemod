//! Behaviour snapshot of the JS-visible `SgRoot` / `SgNode` API.
//!
//! `api_report.js` calls (nearly) every method, including error paths, and
//! returns a JSON report. The expected report in `api_report.snapshot.json`
//! was recorded from the rquickjs implementation that predates the
//! binding-agnostic `sg_core` split, so this test pins "zero behaviour change"
//! for the QuickJS facade. Future facades (napi) can reuse the same script.
//!
//! To re-record (only when a behaviour change is intended), run with
//! `SG_API_SNAPSHOT_OUT=/some/path.json` and copy the file over the snapshot.

use ast_grep_language::SupportLang;
use codemod_sandbox::sandbox::engine::execution_engine::{
    execute_codemod_with_quickjs, CodemodOutput, ExecutionResult, JssgExecutionOptions,
};
use codemod_sandbox::sandbox::engine::selector_from_value;
use codemod_sandbox::sandbox::resolvers::oxc_resolver::OxcResolver;
use codemod_sandbox::CodemodLang;
use std::fs;
use std::sync::Arc;

const INPUT: &str = r#"import { a } from "./a";
const greeting = 'héllo';
console.log(a);
foo(1, "two", three);
console.log(bar);
"#;

#[tokio::test]
async fn sg_api_report_matches_snapshot() {
    let temp_dir = tempfile::tempdir().unwrap();
    let work_dir = temp_dir.path().join("work");
    fs::create_dir_all(&work_dir).unwrap();
    let codemod_path = work_dir.join("codemod.js");
    fs::write(&codemod_path, include_str!("api_report.js")).unwrap();
    let target_path = work_dir.join("input.ts");
    fs::write(&target_path, INPUT).unwrap();

    let language = CodemodLang::Static(SupportLang::TypeScript);
    let selector = selector_from_value(
        language,
        serde_json::json!({ "rule": { "pattern": "console.log($A)" } }),
    )
    .unwrap();

    let options = JssgExecutionOptions {
        script_path: &codemod_path,
        resolver: Arc::new(OxcResolver::new(work_dir.clone(), None).unwrap()),
        language,
        file_path: &target_path,
        content: INPUT,
        selector_config: Some(Arc::new(selector)),
        params: None,
        matrix_values: None,
        capabilities: None,
        semantic_provider: None,
        metrics_context: None,
        llm_request_handler: None,
        shared_state_context: None,
        runtime_event_callback: None,
        cancellation_flag: None,
        test_mode: false,
        dry_run: false,
        stage_writes: false,
        target_directory: &work_dir,
    };

    let modified = match execute_codemod_with_quickjs(options).await {
        Ok(CodemodOutput {
            primary: ExecutionResult::Modified(modified),
            ..
        }) => modified,
        other => panic!("expected a modified result, got {other:?}"),
    };

    let rename_to = modified.rename_to.expect("rename() should be recorded");
    assert!(
        rename_to.ends_with("work/renamed.ts"),
        "unexpected rename target {rename_to:?}"
    );

    // Error messages embed the temp directory; normalize every spelling of it.
    let mut actual = modified.content;
    let canonical = work_dir.canonicalize().unwrap();
    for dir in [canonical.as_path(), work_dir.as_path()] {
        actual = actual.replace(&*dir.to_string_lossy(), "<WORK>");
    }

    if let Ok(out) = std::env::var("SG_API_SNAPSHOT_OUT") {
        fs::write(out, &actual).unwrap();
        return;
    }

    let expected = include_str!("api_report.snapshot.json");
    if actual != expected {
        panic!(
            "SgRoot/SgNode API behaviour changed.\n--- expected\n{expected}\n--- actual\n{actual}"
        );
    }
}
