//! Stateful session behavior: opening, indexing, transforming, and the path
//! validation applied on both directions of the boundary. Every test uses a
//! temporary repository and a separate workflow directory for the script.

use std::path::{Path, PathBuf};

use butterflow_execution_bridge::{
    session::{FileResult, JssgSession, SessionConfig, TransformResult},
    SemanticAnalysis, SemanticAnalysisDetails, SemanticMode,
};
use tempfile::TempDir;

const REPLACE_SCRIPT: &str = r#"export default async function transform(root, options) {
  return {
    content: root.root().text().replaceAll("oldApi", "newApi"),
    output: { file: root.relativeFilename().replaceAll("\\", "/"), input: options.params.input ?? null },
  };
}"#;

fn write(dir: &Path, relative: &str, content: &str) -> PathBuf {
    let path = dir.join(relative);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(&path, content).expect("write");
    path
}

fn read(dir: &Path, relative: &str) -> String {
    std::fs::read_to_string(dir.join(relative)).expect("read")
}

struct Fixture {
    repo: TempDir,
    workflow: TempDir,
}

impl Fixture {
    fn new(script: &str) -> Self {
        let workflow = tempfile::tempdir().expect("workflow dir");
        write(workflow.path(), "transform.js", script);
        Self {
            repo: tempfile::tempdir().expect("repo dir"),
            workflow,
        }
    }

    fn root(&self) -> &Path {
        self.repo.path()
    }

    fn config(&self) -> SessionConfig {
        SessionConfig {
            script: "transform.js".to_string(),
            script_root: self.workflow.path().to_path_buf(),
            language: "typescript".to_string(),
            target_root: self.repo.path().to_path_buf(),
            semantic_analysis: None,
            input: None,
        }
    }

    async fn open(&self) -> JssgSession {
        JssgSession::open(self.config())
            .await
            .expect("session opens")
    }
}

async fn transform(session: &JssgSession, root: &Path, relative: &str) -> TransformResult {
    session
        .transform(relative, &read(root, relative))
        .await
        .unwrap_or_else(|error| panic!("transform '{relative}' failed: {error}"))
}

#[tokio::test]
async fn open_reports_language_extensions_and_semantic_mode() {
    let fixture = Fixture::new(REPLACE_SCRIPT);
    let session = fixture.open().await;
    let info = session.info();
    assert!(info.extensions.contains(&".ts".to_string()), "{info:?}");
    assert!(info.extensions.contains(&".js".to_string()), "{info:?}");
    assert!(!info.extensions.contains(&".tsx".to_string()), "{info:?}");
    assert_eq!(info.semantic_mode, None);

    let mut config = fixture.config();
    config.semantic_analysis = Some(SemanticAnalysis::Mode(SemanticMode::Workspace));
    let workspace = JssgSession::open(config).await.expect("workspace session");
    assert_eq!(
        workspace.info().semantic_mode,
        Some(SemanticMode::Workspace)
    );
    assert_eq!(
        workspace.target_root(),
        fixture.root().canonicalize().expect("canonical root")
    );
}

#[tokio::test]
async fn open_rejects_bad_configuration_without_touching_the_repository() {
    let fixture = Fixture::new(REPLACE_SCRIPT);
    write(fixture.root(), "src/a.ts", "oldApi('a');\n");
    type Mutation = Box<dyn Fn(&mut SessionConfig)>;
    let cases: Vec<(Mutation, &str)> = vec![
        (
            Box::new(|c| c.language = "klingon".to_string()),
            "invalid JSSG language",
        ),
        (
            Box::new(|c| c.script = "missing.js".to_string()),
            "failed to resolve JSSG script",
        ),
        (
            Box::new(|c| c.script = "../transform.js".to_string()),
            "safe relative path",
        ),
        (
            Box::new(|c| c.script_root = PathBuf::from("relative")),
            "scriptRoot must be an absolute path",
        ),
        (
            Box::new(|c| c.target_root = PathBuf::from("relative")),
            "targetRoot must be an absolute path",
        ),
        (
            Box::new(|c| c.target_root = c.target_root.join("nope")),
            "failed to resolve target root",
        ),
        (
            Box::new(|c| {
                c.semantic_analysis = Some(SemanticAnalysis::Detailed(SemanticAnalysisDetails {
                    mode: SemanticMode::Workspace,
                    root: Some("nope".to_string()),
                }))
            }),
            "failed to resolve semanticAnalysis.root",
        ),
        (
            Box::new(|c| {
                c.semantic_analysis = Some(SemanticAnalysis::Detailed(SemanticAnalysisDetails {
                    mode: SemanticMode::File,
                    root: Some("src".to_string()),
                }))
            }),
            "requires workspace mode",
        ),
    ];
    for (mutate, expected) in cases {
        let mut config = fixture.config();
        mutate(&mut config);
        let error = JssgSession::open(config)
            .await
            .err()
            .unwrap_or_else(|| panic!("expected an error containing '{expected}'"));
        assert!(error.contains(expected), "{expected}: {error}");
        assert_eq!(read(fixture.root(), "src/a.ts"), "oldApi('a');\n");
    }
}

#[tokio::test]
async fn selector_errors_fail_open() {
    let fixture = Fixture::new(
        r#"export function getSelector() { throw new Error("selector exploded"); }
export default async function transform() { return null; }"#,
    );
    let error = JssgSession::open(fixture.config())
        .await
        .err()
        .unwrap_or_else(|| panic!("selector error"));
    assert!(error.contains("failed to load JSSG selector"), "{error}");
    assert!(error.contains("selector exploded"), "{error}");
}

#[tokio::test]
async fn transform_returns_edits_and_output_without_writing() {
    let fixture = Fixture::new(REPLACE_SCRIPT);
    write(fixture.root(), "src/a.ts", "oldApi('a');\n");
    let mut config = fixture.config();
    config.input = Some(serde_json::json!({ "marker": 1 }));
    let session = JssgSession::open(config).await.expect("session");

    let result = transform(&session, fixture.root(), "src/a.ts").await;

    assert_eq!(
        result.primary,
        FileResult::Modified {
            content: "newApi('a');\n".to_string(),
            rename_to: None
        }
    );
    assert!(result.secondary.is_empty());
    assert_eq!(
        result.output,
        Some(serde_json::json!({ "file": "src/a.ts", "input": { "marker": 1 } }))
    );
    assert_eq!(read(fixture.root(), "src/a.ts"), "oldApi('a');\n");
}

#[tokio::test]
async fn one_session_transforms_many_files_with_one_loaded_script() {
    let fixture = Fixture::new(REPLACE_SCRIPT);
    write(fixture.root(), "src/a.ts", "oldApi('a');\n");
    write(fixture.root(), "src/b.ts", "fine();\n");
    let session = fixture.open().await;

    let a = transform(&session, fixture.root(), "src/a.ts").await;
    let b = transform(&session, fixture.root(), "src/b.ts").await;

    assert!(matches!(a.primary, FileResult::Modified { .. }));
    assert_eq!(b.primary, FileResult::Unmodified);
    assert_eq!(
        b.output,
        Some(serde_json::json!({ "file": "src/b.ts", "input": null }))
    );
}

#[tokio::test]
async fn legacy_string_and_null_results_carry_no_output() {
    let fixture = Fixture::new(
        r#"export default async function transform(root) {
  const text = root.root().text();
  return text.includes("skip") ? null : text.replaceAll("oldApi", "newApi");
}"#,
    );
    write(fixture.root(), "a.ts", "oldApi('a');\n");
    write(fixture.root(), "b.ts", "skip();\n");
    let session = fixture.open().await;

    let a = transform(&session, fixture.root(), "a.ts").await;
    let b = transform(&session, fixture.root(), "b.ts").await;

    assert_eq!(
        a.primary,
        FileResult::Modified {
            content: "newApi('a');\n".to_string(),
            rename_to: None
        }
    );
    assert_eq!(a.output, None);
    assert_eq!(b.primary, FileResult::Unmodified);
    assert_eq!(b.output, None);
}

#[tokio::test]
async fn selector_skips_files_without_matches() {
    let fixture = Fixture::new(
        r#"export function getSelector() { return { rule: { pattern: "oldApi($A)" } }; }
export default async function transform(root) { return root.root().text().replaceAll("oldApi", "newApi"); }"#,
    );
    write(fixture.root(), "a.ts", "oldApi('a');\n");
    write(fixture.root(), "b.ts", "other();\n");
    let session = fixture.open().await;

    assert!(matches!(
        transform(&session, fixture.root(), "a.ts").await.primary,
        FileResult::Modified { .. }
    ));
    assert_eq!(
        transform(&session, fixture.root(), "b.ts").await.primary,
        FileResult::Skipped
    );
}

#[tokio::test]
async fn transform_errors_are_returned_and_leave_the_session_usable() {
    let fixture = Fixture::new(
        r#"export default async function transform(root) {
  if (root.relativeFilename().endsWith("b.ts")) throw new Error("second file");
  return root.root().text().replaceAll("oldApi", "newApi");
}"#,
    );
    write(fixture.root(), "a.ts", "oldApi('a');\n");
    write(fixture.root(), "b.ts", "oldApi('b');\n");
    let session = fixture.open().await;

    let error = session
        .transform("b.ts", "oldApi('b');\n")
        .await
        .expect_err("transform error");
    assert!(error.contains("JSSG failed for 'b.ts'"), "{error}");
    assert!(error.contains("second file"), "{error}");
    assert!(matches!(
        transform(&session, fixture.root(), "a.ts").await.primary,
        FileResult::Modified { .. }
    ));
    assert_eq!(read(fixture.root(), "b.ts"), "oldApi('b');\n");
}

#[tokio::test]
async fn rename_targets_come_back_root_relative_in_wire_form() {
    let fixture = Fixture::new(
        r#"export default async function transform(root, options) {
  const file = root.relativeFilename().replaceAll("\\", "/");
  if (file === "src/a.ts") root.rename("moved/a.ts");
  if (file === "src/b.ts") root.rename(options.targetDir + "/top/b.ts");
  return null;
}"#,
    );
    write(fixture.root(), "src/a.ts", "a();\n");
    write(fixture.root(), "src/b.ts", "b();\n");
    let session = fixture.open().await;

    let a = transform(&session, fixture.root(), "src/a.ts").await;
    let b = transform(&session, fixture.root(), "src/b.ts").await;

    // A relative rename resolves against the file's directory; an absolute
    // one against nothing. Both come back relative to the target root.
    assert_eq!(
        a.primary,
        FileResult::Modified {
            content: "a();\n".to_string(),
            rename_to: Some("src/moved/a.ts".to_string())
        }
    );
    assert_eq!(
        b.primary,
        FileResult::Modified {
            content: "b();\n".to_string(),
            rename_to: Some("top/b.ts".to_string())
        }
    );
    assert!(fixture.root().join("src/a.ts").exists());
    assert!(!fixture.root().join("src/moved").exists());
}

#[tokio::test]
async fn secondary_results_from_jssg_transform_are_root_relative_and_unwritten() {
    let fixture = Fixture::new(
        r#"import { jssgTransform } from "codemod:ast-grep";
export default async function transform(root, options) {
  await jssgTransform(
    async (secondary) => { secondary.rename("moved/b.ts"); return secondary.root().text().replaceAll("oldApi", "newApi"); },
    options.targetDir + "/src/b.ts",
    "typescript",
  );
  return null;
}"#,
    );
    write(fixture.root(), "src/a.ts", "a();\n");
    write(fixture.root(), "src/b.ts", "oldApi('b');\n");
    let session = fixture.open().await;

    let result = transform(&session, fixture.root(), "src/a.ts").await;

    assert_eq!(result.primary, FileResult::Unmodified);
    assert_eq!(result.secondary.len(), 1);
    assert_eq!(result.secondary[0].path, "src/b.ts");
    assert_eq!(
        result.secondary[0].result,
        FileResult::Modified {
            content: "newApi('b');\n".to_string(),
            rename_to: Some("src/moved/b.ts".to_string())
        }
    );
    assert_eq!(read(fixture.root(), "src/b.ts"), "oldApi('b');\n");
}

#[tokio::test]
async fn sandbox_rejects_renames_and_secondary_targets_outside_the_root() {
    let fixture = Fixture::new(
        r#"import { jssgTransform } from "codemod:ast-grep";
export default async function transform(root, options) {
  const file = root.relativeFilename().replaceAll("\\", "/");
  if (file === "rename.ts") root.rename("../escaped.ts");
  if (file === "secondary.ts") {
    await jssgTransform(async () => "x", options.targetDir + "/../outside.ts", "typescript");
  }
  return null;
}"#,
    );
    write(fixture.root(), "rename.ts", "a();\n");
    write(fixture.root(), "secondary.ts", "b();\n");
    let session = fixture.open().await;

    for file in ["rename.ts", "secondary.ts"] {
        let error = session
            .transform(file, "x();\n")
            .await
            .err()
            .unwrap_or_else(|| panic!("{file} must be rejected"));
        assert!(
            error.contains("outside the target directory"),
            "{file}: {error}"
        );
    }
    assert!(!fixture
        .root()
        .parent()
        .expect("parent")
        .join("escaped.ts")
        .exists());
    assert!(!fixture
        .root()
        .parent()
        .expect("parent")
        .join("outside.ts")
        .exists());
}

#[tokio::test]
async fn source_paths_must_stay_inside_the_target_root() {
    let fixture = Fixture::new(REPLACE_SCRIPT);
    write(fixture.root(), "src/a.ts", "a();\n");
    let session = fixture.open().await;

    for bad in [
        "../a.ts",
        "/abs/a.ts",
        "src/../../a.ts",
        "C:\\a.ts",
        "\\\\server\\a.ts",
        " ",
    ] {
        let error = session
            .transform(bad, "a();\n")
            .await
            .err()
            .unwrap_or_else(|| panic!("{bad:?} must be rejected"));
        assert!(
            error.contains("safe relative path") || error.contains("must not be empty"),
            "{bad:?}: {error}"
        );
        let index_error = session
            .index(bad, "a();\n")
            .err()
            .unwrap_or_else(|| panic!("{bad:?} must be rejected by index"));
        assert!(
            index_error.contains("safe relative path") || index_error.contains("must not be empty"),
            "{bad:?}: {index_error}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_that_leave_the_root_are_rejected_on_both_directions() {
    let fixture = Fixture::new(
        r#"export default async function transform(root) {
  if (root.relativeFilename().includes("escape")) root.rename("escape/out.ts");
  return root.root().text() + "// touched\n";
}"#,
    );
    let outside = tempfile::tempdir().expect("outside");
    write(outside.path(), "secret.ts", "secret();\n");
    std::fs::create_dir_all(outside.path().join("dir")).expect("outside dir");
    std::os::unix::fs::symlink(
        outside.path().join("secret.ts"),
        fixture.root().join("link.ts"),
    )
    .expect("file symlink");
    std::os::unix::fs::symlink(outside.path().join("dir"), fixture.root().join("escape"))
        .expect("dir symlink");
    write(fixture.root(), "escape-me.ts", "a();\n");
    let session = fixture.open().await;

    // A symlinked source file resolves outside the root.
    let error = session
        .transform("link.ts", "secret();\n")
        .await
        .expect_err("symlinked source rejected");
    assert!(error.contains("escapes the target root"), "{error}");
    let error = session
        .index("link.ts", "secret();\n")
        .expect_err("symlinked index rejected");
    assert!(error.contains("escapes the target root"), "{error}");

    // A rename into a symlinked directory that points outside is rejected by
    // the sandbox's own check and would be by the session's normalization.
    let error = session
        .transform("escape-me.ts", "a();\n")
        .await
        .expect_err("rename through symlink rejected");
    assert!(
        error.contains("outside the target directory") || error.contains("escapes the target root"),
        "{error}"
    );
    assert!(!outside.path().join("dir/out.ts").exists());
    assert_eq!(read(outside.path(), "secret.ts"), "secret();\n");
}

#[tokio::test]
async fn workspace_semantics_share_one_provider_across_files_and_stage_writes() {
    let fixture = Fixture::new(
        r#"export default async function transform(root) {
  const calls = root.root().findAll({ rule: { pattern: "add" } });
  let call = null;
  for (const node of calls) {
    const parent = node.parent();
    if (parent && parent.kind() === "call_expression") { call = node; break; }
  }
  if (!call) return { content: null, output: { file: root.relativeFilename(), definition: null } };
  let definition = call.definition();
  for (let hop = 0; definition && definition.root.filename() === root.filename() && hop < 3; hop++) {
    definition = definition.node.definition();
  }
  if (!definition) return { content: null, output: { file: root.relativeFilename(), definition: null } };
  definition.root.write(definition.root.root().text().replace("add", "sum"));
  return { content: null, output: { file: root.relativeFilename(), definition: definition.root.relativeFilename() } };
}"#,
    );
    write(
        fixture.root(),
        "main.ts",
        "import { add } from \"./utils\";\nconst result = add(1, 2);\n",
    );
    write(
        fixture.root(),
        "utils.ts",
        "export function add(a: number, b: number): number {\n  return a + b;\n}\n",
    );
    let mut config = fixture.config();
    config.semantic_analysis = Some(SemanticAnalysis::Mode(SemanticMode::Workspace));
    let session = JssgSession::open(config).await.expect("session");
    for file in ["main.ts", "utils.ts"] {
        session
            .index(file, &read(fixture.root(), file))
            .expect("index");
    }

    let result = transform(&session, fixture.root(), "main.ts").await;

    assert_eq!(
        result.output,
        Some(serde_json::json!({ "file": "main.ts", "definition": "utils.ts" }))
    );
    assert_eq!(result.primary, FileResult::Unmodified);
    assert_eq!(result.secondary.len(), 1, "{:?}", result.secondary);
    assert_eq!(result.secondary[0].path, "utils.ts");
    assert!(matches!(
        &result.secondary[0].result,
        FileResult::Modified { content, rename_to: None } if content.contains("function sum")
    ));
    // The session never writes: utils.ts on disk is untouched.
    assert!(read(fixture.root(), "utils.ts").contains("function add"));
}
