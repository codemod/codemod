//! `run_callback_file`: a callback file run on one file, as `forEachFile`
//! runs it, without committing what it does.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use codemod_sandbox::sandbox::resolvers::OxcResolver;
use codemod_workflow::{run_callback_file, CallbackFileRequest, CallbackFileRun};
use tempfile::TempDir;

fn run(dir: &Path, callback: &str, path: &str, text: &str) -> Result<CallbackFileRun, String> {
    std::fs::write(dir.join("callback.ts"), callback).unwrap();
    let request = CallbackFileRequest {
        callback: dir.join("callback.ts"),
        resolver: Arc::new(OxcResolver::new(dir.to_path_buf(), None).unwrap()),
        target: dir.to_path_buf(),
        path: path.to_string(),
        text: text.to_string(),
        language: Some("typescript".to_string()),
        semantic_provider: None,
        capabilities: HashSet::new(),
        runtime_event_callback: None,
    };
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(run_callback_file(request))
}

#[test]
fn the_last_write_and_the_return_value_come_back() {
    let dir = TempDir::new().unwrap();
    let run = run(
        dir.path(),
        r#"
import { parse } from "codemod/ast-grep";
export default (file: any) => {
  file.write("first");
  file.write(parse(file).root().text().replace("var", "let"));
  return { path: file.path, language: file.language };
};
"#,
        "src/a.ts",
        "var a = 1;",
    )
    .unwrap();
    assert_eq!(run.written.as_deref(), Some("let a = 1;"));
    assert_eq!(
        run.output,
        Some(serde_json::json!({ "path": "src/a.ts", "language": "typescript" }))
    );
    assert!(run.renamed.is_none() && !run.deleted);
}

#[test]
fn a_file_left_alone_returns_nothing() {
    let dir = TempDir::new().unwrap();
    let run = run(dir.path(), "export default () => {};", "a.ts", "x").unwrap();
    assert_eq!(run, CallbackFileRun::default());
}

#[test]
fn renames_and_deletes_are_reported() {
    let dir = TempDir::new().unwrap();
    let run = run(
        dir.path(),
        "export default (file: any) => { file.rename('lib/b.ts'); file.delete(); };",
        "a.ts",
        "x",
    )
    .unwrap();
    assert_eq!(run.renamed.as_deref(), Some("lib/b.ts"));
    assert!(run.deleted);
}

#[test]
fn a_thrown_error_or_a_failed_file_is_an_error() {
    let dir = TempDir::new().unwrap();
    let thrown = run(
        dir.path(),
        "export default () => { throw new Error('boom'); };",
        "a.ts",
        "x",
    );
    assert!(thrown.unwrap_err().contains("boom"));
    let failed = run(
        dir.path(),
        "import { failFile } from 'codemod/runtime';\nexport default () => { failFile('not this one'); };",
        "a.ts",
        "x",
    );
    assert!(failed.unwrap_err().contains("not this one"));
}

#[test]
fn a_default_export_that_is_not_a_function_is_refused() {
    let dir = TempDir::new().unwrap();
    let error = run(
        dir.path(),
        "export default { transform() {} };",
        "a.ts",
        "x",
    )
    .unwrap_err();
    assert!(
        error.contains("must default-export a forEachFile callback"),
        "{error}"
    );
}

/// The callback runs from its own file whatever it is called, and whatever
/// directory the resolver starts from.
#[test]
fn a_callback_named_like_the_wrapper_still_runs_from_its_directory() {
    let dir = TempDir::new().unwrap();
    let elsewhere = TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("scripts")).unwrap();
    std::fs::write(
        dir.path().join("scripts/helper.ts"),
        "export const suffix = \"!\";\n",
    )
    .unwrap();
    for name in ["__codemod_callback_file.js", "migrate.ts"] {
        let callback = dir.path().join("scripts").join(name);
        std::fs::write(
            &callback,
            "import { suffix } from \"./helper.ts\";\nexport default (file) => { file.write(file.text + suffix); };\n",
        )
        .unwrap();
        let request = CallbackFileRequest {
            callback,
            resolver: Arc::new(OxcResolver::new(elsewhere.path().to_path_buf(), None).unwrap()),
            target: dir.path().to_path_buf(),
            path: "a.ts".to_string(),
            text: "x".to_string(),
            language: Some("typescript".to_string()),
            semantic_provider: None,
            capabilities: HashSet::new(),
            runtime_event_callback: None,
        };
        let run = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(run_callback_file(request))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(run.written.as_deref(), Some("x!"), "{name}");
    }
}
