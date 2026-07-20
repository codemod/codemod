//! End-to-end regression tests for the web preview's file-change reporting.
//!
//! These run a real `Workflow` through `Engine::run_workflow` with a JS
//! ast-grep step and capture the `DryRunChange` events emitted through the
//! `dry_run_callback`, which is the exact path `crates/cli/src/engine.rs`
//! uses to build the diffs shown in the report/web preview. Covers:
//! - Unchanged files must not appear in the change list at all.
//! - Genuinely modified files are reported with `ChangeKind::Modified`.
//! - Deleted files (via the `fs` capability) are reported with
//!   `ChangeKind::Deleted`.
//! - Renamed files (via `root.rename()`) are reported with
//!   `ChangeKind::Renamed` and the correct `old_path`/new path, instead of
//!   an unrelated delete+create pair.
//! - Moved files (renamed to a different directory) are reported the same
//!   way as renames.

use butterflow_core::config::{DryRunChange, WorkflowRunConfig};
use butterflow_core::diff::ChangeKind;
use butterflow_core::engine::Engine;
use butterflow_core::{Node, Runtime, RuntimeType, Workflow, WorkflowStatus};
use butterflow_models::node::NodeType;
use butterflow_models::step::{
    SemanticAnalysisConfig, SemanticAnalysisMode, StepAction, UseJSAstGrep,
};
use butterflow_models::Step;
use butterflow_state::mock_adapter::MockStateAdapter;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn create_test_file(dir: &Path, name: &str, content: &str) -> PathBuf {
    let file_path = dir.join(name);
    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&file_path, content).unwrap();
    file_path
}

async fn wait_for_completion(engine: &Engine, run_id: uuid::Uuid) {
    let start = Instant::now();
    loop {
        let status = engine.get_workflow_status(run_id).await.unwrap();
        if matches!(
            status,
            WorkflowStatus::Completed | WorkflowStatus::Failed | WorkflowStatus::Canceled
        ) {
            assert_eq!(
                status,
                WorkflowStatus::Completed,
                "workflow did not complete successfully"
            );
            return;
        }
        if start.elapsed() > Duration::from_secs(30) {
            panic!("workflow did not complete in time, last status: {status:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn build_workflow(
    js_file: &str,
    capabilities: Option<Vec<String>>,
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
) -> Workflow {
    Workflow {
        version: "1".to_string(),
        state: None,
        params: None,
        templates: vec![],
        nodes: vec![Node {
            id: "transform".to_string(),
            name: "Transform".to_string(),
            description: None,
            r#type: NodeType::Automatic,
            depends_on: vec![],
            trigger: None,
            strategy: None,
            runtime: Some(Runtime {
                r#type: RuntimeType::Direct,
                image: None,
                working_dir: None,
                user: None,
                network: None,
                options: None,
            }),
            steps: vec![Step {
                id: Some("jssg".to_string()),
                name: "Run JSSG transform".to_string(),
                action: StepAction::JSAstGrep(UseJSAstGrep {
                    js_file: js_file.to_string(),
                    base_path: None,
                    include,
                    exclude,
                    max_threads: Some(1),
                    dry_run: Some(false),
                    language: Some("javascript".to_string()),
                    capabilities,
                    semantic_analysis: Some(SemanticAnalysisConfig::Mode(
                        SemanticAnalysisMode::File,
                    )),
                }),
                env: None,
                condition: None,
                commit: None,
            }],
            env: HashMap::new(),
            branch_name: None,
            pull_request: None,
        }],
    }
}

async fn run_and_collect(temp_path: &Path, workflow: Workflow) -> Vec<DryRunChange> {
    let collected: Arc<Mutex<Vec<DryRunChange>>> = Arc::new(Mutex::new(Vec::new()));
    let collected_for_cb = collected.clone();

    let mut config = WorkflowRunConfig::default();
    config.execution.target_path = temp_path.to_path_buf();
    config.execution.bundle_path = temp_path.to_path_buf();
    config.output.dry_run_callback = Some(Arc::new(move |change: DryRunChange| {
        collected_for_cb.lock().unwrap().push(change);
    }));

    let state_adapter = Box::new(MockStateAdapter::new());
    let engine = Engine::with_state_adapter(state_adapter, config);

    let run_id = engine
        .run_workflow(
            workflow,
            HashMap::new(),
            Some(temp_path.to_path_buf()),
            None,
        )
        .await
        .unwrap();

    wait_for_completion(&engine, run_id).await;

    let result = collected.lock().unwrap().clone();
    result
}

/// Files that a JS ast-grep transform does not touch at all must never be
/// reported as changed, even though every file in the target dir is
/// visited and re-serialized during the walk.
#[tokio::test]
async fn unmodified_file_is_not_reported_as_changed() {
    let temp_dir = tempfile::tempdir().unwrap();
    let temp_path = temp_dir.path();

    create_test_file(
        temp_path,
        "codemod.js",
        r#"
export default function transform(root) {
  const rootNode = root.root();
  const nodes = rootNode.findAll({ rule: { pattern: 'var $X = $Y' } });
  const edits = nodes.map((node) => {
    const x = node.getMatch('X').text();
    const y = node.getMatch('Y').text();
    return node.replace(`const ${x} = ${y}`);
  });
  return rootNode.commitEdits(edits);
}
"#,
    );

    create_test_file(temp_path, "a.js", "var x = 1;\n");
    create_test_file(temp_path, "b.js", "const y = 2;\n");
    // Leading blank line: regression coverage for the tree-sitter root-range
    // trivia bug that previously made this report as falsely modified.
    create_test_file(temp_path, "c.js", "\nconst z = 3;\n");

    let workflow = build_workflow(
        "codemod.js",
        None,
        None,
        Some(vec!["codemod.js".to_string()]),
    );
    let changes = run_and_collect(temp_path, workflow).await;

    let touched: Vec<_> = changes
        .iter()
        .map(|c| {
            c.file_path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .to_string()
        })
        .collect();

    assert!(
        !touched.contains(&"b.js".to_string()),
        "b.js was not modified by the codemod and should not appear in diff callback, got: {touched:?}"
    );
    assert!(
        !touched.contains(&"c.js".to_string()),
        "c.js was not modified by the codemod and should not appear in diff callback, got: {touched:?}"
    );
    assert!(
        !touched.contains(&"codemod.js".to_string()),
        "codemod.js itself is not a transform target and should never appear, got: {touched:?}"
    );

    assert_eq!(
        fs::read_to_string(temp_path.join("a.js")).unwrap(),
        "const x = 1\n"
    );
    assert_eq!(
        fs::read_to_string(temp_path.join("b.js")).unwrap(),
        "const y = 2;\n"
    );
    assert_eq!(
        fs::read_to_string(temp_path.join("c.js")).unwrap(),
        "\nconst z = 3;\n"
    );
}

/// A file whose content genuinely changes must be reported once, with
/// `ChangeKind::Modified` and no rename metadata.
#[tokio::test]
async fn modified_file_is_reported_as_modified() {
    let temp_dir = tempfile::tempdir().unwrap();
    let temp_path = temp_dir.path();

    create_test_file(
        temp_path,
        "codemod.js",
        r#"
export default function transform(root) {
  const rootNode = root.root();
  const nodes = rootNode.findAll({ rule: { pattern: 'var $X = $Y' } });
  const edits = nodes.map((node) => {
    const x = node.getMatch('X').text();
    const y = node.getMatch('Y').text();
    return node.replace(`const ${x} = ${y}`);
  });
  return rootNode.commitEdits(edits);
}
"#,
    );
    create_test_file(temp_path, "a.js", "var x = 1;\n");

    let workflow = build_workflow(
        "codemod.js",
        None,
        None,
        Some(vec!["codemod.js".to_string()]),
    );
    let changes = run_and_collect(temp_path, workflow).await;

    let a_changes: Vec<_> = changes
        .iter()
        .filter(|c| c.file_path.file_name().unwrap() == "a.js")
        .collect();
    assert_eq!(
        a_changes.len(),
        1,
        "expected exactly one change for a.js, got {a_changes:?}"
    );
    let change = a_changes[0];
    assert_eq!(change.kind, ChangeKind::Modified);
    assert!(change.new_path.is_none());
    assert_eq!(change.original_content, "var x = 1;\n");
    assert_eq!(change.new_content, "const x = 1\n");

    assert_eq!(
        fs::read_to_string(temp_path.join("a.js")).unwrap(),
        "const x = 1\n"
    );
}

/// A file removed by the codemod (via the sandboxed `fs` module) must be
/// reported as `ChangeKind::Deleted`, even though the engine's execution
/// result for that file is `Unmodified`/`Skipped` from the transform's own
/// point of view.
#[tokio::test]
async fn deleted_file_is_reported_as_deleted() {
    let temp_dir = tempfile::tempdir().unwrap();
    let temp_path = temp_dir.path();

    create_test_file(
        temp_path,
        "codemod.js",
        r#"
import fs from "fs";

export default function transform(root) {
  fs.unlinkSync(root.filename());
  return null;
}
"#,
    );
    create_test_file(temp_path, "doomed.js", "var x = 1;\n");

    let workflow = build_workflow(
        "codemod.js",
        None,
        None,
        Some(vec!["codemod.js".to_string()]),
    );
    let changes = run_and_collect(temp_path, workflow).await;

    let doomed_changes: Vec<_> = changes
        .iter()
        .filter(|c| c.file_path.file_name().unwrap() == "doomed.js")
        .collect();
    assert_eq!(
        doomed_changes.len(),
        1,
        "expected exactly one change for doomed.js, got {doomed_changes:?}"
    );
    let change = doomed_changes[0];
    assert_eq!(change.kind, ChangeKind::Deleted);
    assert_eq!(change.original_content, "var x = 1;\n");

    assert!(!temp_path.join("doomed.js").exists());
}

/// A file renamed via `root.rename()` (same directory) must be reported as
/// `ChangeKind::Renamed` with `old_path` set to the original location and
/// `new_path` set to the destination, instead of an unrelated delete and an
/// unrelated create.
#[tokio::test]
async fn renamed_file_is_reported_as_renamed() {
    let temp_dir = tempfile::tempdir().unwrap();
    let temp_path = temp_dir.path();

    create_test_file(
        temp_path,
        "codemod.js",
        r#"
export default function transform(root) {
  root.rename(root.filename().replace(".eslintrc", "eslint.config.cjs"));
  return null;
}
"#,
    );
    create_test_file(temp_path, ".eslintrc", "{}\n");

    // The default JS include glob only matches `.js`-family extensions, so
    // the dotfile with no extension needs an explicit include pattern.
    let workflow = build_workflow(
        "codemod.js",
        None,
        Some(vec![".eslintrc".to_string()]),
        None,
    );
    let changes = run_and_collect(temp_path, workflow).await;
    // `root.rename()` resolves relative destinations via the canonicalized
    // parent directory, so the new path may not share the exact (possibly
    // symlinked) prefix of the original, uncanonicalized walker path.
    let canonical_temp_path = fs::canonicalize(temp_path).unwrap();

    assert_eq!(
        changes.len(),
        1,
        "expected exactly one change event for the rename, got {changes:?}"
    );
    let change = &changes[0];
    assert_eq!(change.kind, ChangeKind::Renamed);
    assert_eq!(change.file_path, temp_path.join(".eslintrc"));
    assert_eq!(
        change.new_path.as_deref(),
        Some(canonical_temp_path.join("eslint.config.cjs").as_path())
    );
    assert_eq!(change.original_content, "{}\n");
    assert_eq!(change.new_content, "{}\n");

    assert!(!temp_path.join(".eslintrc").exists());
    assert_eq!(
        fs::read_to_string(temp_path.join("eslint.config.cjs")).unwrap(),
        "{}\n"
    );
}

/// A file moved to a different directory (still via `root.rename()`) must be
/// reported the same way as an in-place rename: `ChangeKind::Renamed` with
/// the full old and new paths.
#[tokio::test]
async fn moved_file_is_reported_as_renamed() {
    let temp_dir = tempfile::tempdir().unwrap();
    let temp_path = temp_dir.path();

    create_test_file(
        temp_path,
        "codemod.js",
        r#"
export default function transform(root) {
  root.rename("config/settings.js");
  return null;
}
"#,
    );
    create_test_file(temp_path, "settings.js", "module.exports = {};\n");

    let workflow = build_workflow(
        "codemod.js",
        None,
        None,
        Some(vec!["codemod.js".to_string()]),
    );
    let changes = run_and_collect(temp_path, workflow).await;
    let canonical_temp_path = fs::canonicalize(temp_path).unwrap();

    assert_eq!(
        changes.len(),
        1,
        "expected exactly one change event for the move, got {changes:?}"
    );
    let change = &changes[0];
    assert_eq!(change.kind, ChangeKind::Renamed);
    assert_eq!(change.file_path, temp_path.join("settings.js"));
    assert_eq!(
        change.new_path.as_deref(),
        Some(canonical_temp_path.join("config/settings.js").as_path())
    );

    assert!(!temp_path.join("settings.js").exists());
    assert_eq!(
        fs::read_to_string(temp_path.join("config/settings.js")).unwrap(),
        "module.exports = {};\n"
    );
}

/// A rename that also changes content must report the new content, not the
/// unrenamed original, while still carrying `ChangeKind::Renamed`.
#[tokio::test]
async fn renamed_file_with_content_change_reports_new_content() {
    let temp_dir = tempfile::tempdir().unwrap();
    let temp_path = temp_dir.path();

    create_test_file(
        temp_path,
        "codemod.js",
        r#"
export default function transform(root) {
  const rootNode = root.root();
  root.rename(root.filename().replace(".js", ".mjs"));
  const nodes = rootNode.findAll({ rule: { pattern: 'var $X = $Y' } });
  const edits = nodes.map((node) => {
    const x = node.getMatch('X').text();
    const y = node.getMatch('Y').text();
    return node.replace(`const ${x} = ${y}`);
  });
  return rootNode.commitEdits(edits);
}
"#,
    );
    create_test_file(temp_path, "legacy.js", "var x = 1;\n");

    let workflow = build_workflow(
        "codemod.js",
        None,
        None,
        Some(vec!["codemod.js".to_string()]),
    );
    let changes = run_and_collect(temp_path, workflow).await;

    assert_eq!(
        changes.len(),
        1,
        "expected exactly one change event, got {changes:?}"
    );
    let change = &changes[0];
    assert_eq!(change.kind, ChangeKind::Renamed);
    assert_eq!(change.original_content, "var x = 1;\n");
    assert_eq!(change.new_content, "const x = 1\n");

    assert!(!temp_path.join("legacy.js").exists());
    assert_eq!(
        fs::read_to_string(temp_path.join("legacy.mjs")).unwrap(),
        "const x = 1\n"
    );
}
