//! One JSSG batch through the real sandbox: outcomes as data, one shared
//! runtime configuration and semantic provider, and path validation on both
//! directions of the boundary. Every test uses a temporary repository and a
//! separate workflow directory; nothing on disk changes.

use std::path::{Path, PathBuf};

use butterflow_execution_bridge::{
    jssg::{transform_batch, Batch, Edit, FileOutcome},
    BatchFile, SemanticAnalysis, SemanticAnalysisDetails, SemanticMode,
};
use serde_json::{json, Value};
use tempfile::TempDir;

const REPLACE: &str = r#"export default async function transform(root, options) {
  const text = root.root().text();
  if (text.includes("boom")) throw new Error("boom");
  return {
    content: text.replaceAll("oldApi", "newApi"),
    output: { file: root.relativeFilename().replaceAll("\\", "/"), input: options.params.input ?? null },
  };
}"#;

struct Fixture {
    repo: TempDir,
    workflow: TempDir,
    files: Vec<BatchFile>,
    semantic: Option<SemanticAnalysis>,
    input: Option<Value>,
}

impl Fixture {
    fn new(script: &str, files: &[(&str, &str)]) -> Self {
        let workflow = tempfile::tempdir().expect("workflow dir");
        std::fs::write(workflow.path().join("transform.js"), script).expect("script");
        let repo = tempfile::tempdir().expect("repo dir");
        let files = files
            .iter()
            .map(|(path, content)| {
                write(repo.path(), path, content);
                BatchFile {
                    path: path.to_string(),
                    content: content.to_string(),
                }
            })
            .collect();
        Self {
            repo,
            workflow,
            files,
            semantic: None,
            input: None,
        }
    }

    fn root(&self) -> &Path {
        self.repo.path()
    }

    async fn run_with(
        &self,
        script_root: &Path,
        target_root: &Path,
        script: &str,
        language: &str,
        files: &[BatchFile],
    ) -> Result<Vec<FileOutcome>, String> {
        transform_batch(Batch {
            script,
            script_root: script_root.to_str(),
            language,
            target_root: target_root.to_str(),
            semantic_analysis: self.semantic.as_ref(),
            input: self.input.as_ref(),
            files,
        })
        .await
    }

    async fn run(&self) -> Result<Vec<FileOutcome>, String> {
        self.run_with(
            self.workflow.path(),
            self.root(),
            "transform.js",
            "typescript",
            &self.files,
        )
        .await
    }

    /// Every file on disk still holds the content the batch was given.
    fn assert_untouched(&self) {
        for file in &self.files {
            assert_eq!(read(self.root(), &file.path), file.content, "{}", file.path);
        }
    }
}

fn write(dir: &Path, relative: &str, content: &str) -> PathBuf {
    let path = dir.join(relative);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(&path, content).expect("write");
    path
}

fn read(dir: &Path, relative: &str) -> String {
    std::fs::read_to_string(dir.join(relative)).expect("read")
}

fn edit(path: &str, content: &str, rename_to: Option<&str>) -> Edit {
    Edit {
        path: path.to_string(),
        content: content.to_string(),
        rename_to: rename_to.map(str::to_string),
    }
}

#[tokio::test]
async fn a_batch_returns_edits_and_outputs_in_order_without_writing() {
    let mut fixture = Fixture::new(
        REPLACE,
        &[("src/a.ts", "oldApi('a');\n"), ("src/b.ts", "fine();\n")],
    );
    fixture.input = Some(json!({ "marker": 1 }));

    let outcomes = fixture.run().await.expect("batch");

    assert_eq!(
        outcomes,
        vec![
            FileOutcome {
                path: "src/a.ts".to_string(),
                edits: vec![edit("src/a.ts", "newApi('a');\n", None)],
                output: Some(json!({ "file": "src/a.ts", "input": { "marker": 1 } })),
            },
            FileOutcome {
                path: "src/b.ts".to_string(),
                edits: vec![],
                output: Some(json!({ "file": "src/b.ts", "input": { "marker": 1 } })),
            },
        ]
    );
    fixture.assert_untouched();
}

#[tokio::test]
async fn legacy_returns_and_selectors_behave_as_in_the_engine() {
    // string | null returns carry no output; a selector skips non-matching files.
    let fixture = Fixture::new(
        r#"export function getSelector() { return { rule: { pattern: "oldApi($A)" } }; }
export default async function transform(root) {
  const text = root.root().text();
  return text.includes("skip") ? null : text.replaceAll("oldApi", "newApi");
}"#,
        &[
            ("a.ts", "oldApi('a');\n"),
            ("skip.ts", "oldApi('skip');\n"),
            ("other.ts", "other();\n"),
        ],
    );
    let outcomes = fixture.run().await.expect("batch");
    assert_eq!(
        outcomes
            .iter()
            .map(|outcome| (outcome.edits.len(), outcome.output.is_none()))
            .collect::<Vec<_>>(),
        vec![(1, true), (0, true), (0, true)]
    );
    assert_eq!(outcomes[0].edits[0].content, "newApi('a');\n");
}

#[tokio::test]
async fn renames_and_secondary_edits_come_back_root_relative() {
    let fixture = Fixture::new(
        r#"import { jssgTransform } from "codemod:ast-grep";
export default async function transform(root, options) {
  const file = root.relativeFilename().replaceAll("\\", "/");
  if (file === "src/a.ts") { root.rename("moved/a.ts"); return "a();\n"; }
  if (file === "src/b.ts") { root.rename(options.targetDir + "/top/b.ts"); return "b();\n"; }
  if (file !== "src/c.ts") return null;
  await jssgTransform(
    async (secondary) => { secondary.rename("moved/d.ts"); return secondary.root().text().replaceAll("oldApi", "newApi"); },
    options.targetDir + "/src/d.ts",
    "typescript",
  );
  return null;
}"#,
        &[
            ("src/a.ts", "a();\n"),
            ("src/b.ts", "b();\n"),
            ("src/c.ts", "c();\n"),
            ("src/d.ts", "oldApi('d');\n"),
        ],
    );

    let outcomes = fixture.run().await.expect("batch");

    // A relative rename resolves against the file's directory, an absolute
    // one against nothing; both come back relative to the target root.
    assert_eq!(
        outcomes
            .iter()
            .flat_map(|outcome| outcome.edits.clone())
            .collect::<Vec<_>>(),
        vec![
            edit("src/a.ts", "a();\n", Some("src/moved/a.ts")),
            edit("src/b.ts", "b();\n", Some("top/b.ts")),
            edit("src/d.ts", "newApi('d');\n", Some("src/moved/d.ts")),
        ]
    );
    fixture.assert_untouched();
    assert!(!fixture.root().join("src/moved").exists());
}

#[tokio::test]
async fn setup_and_transform_failures_fail_the_batch_before_any_result() {
    let fixture = Fixture::new(
        REPLACE,
        &[("src/a.ts", "oldApi('a');\n"), ("src/b.ts", "boom();\n")],
    );
    let error = fixture.run().await.expect_err("second file throws");
    assert!(
        error.contains("JSSG failed for 'src/b.ts'") && error.contains("boom"),
        "{error}"
    );

    let workflow = fixture.workflow.path();
    let root = fixture.root();
    let missing_root = root.join("nope");
    let cases: Vec<(&Path, &Path, &str, &str, &str)> = vec![
        // (script root, target root, script, language, expected error)
        (
            workflow,
            root,
            "transform.js",
            "klingon",
            "invalid JSSG language",
        ),
        (
            workflow,
            root,
            "missing.js",
            "typescript",
            "failed to resolve JSSG script",
        ),
        (
            workflow,
            root,
            "../transform.js",
            "typescript",
            "safe relative path",
        ),
        (
            Path::new("relative"),
            root,
            "transform.js",
            "typescript",
            "scriptRoot must be an absolute path",
        ),
        (
            workflow,
            Path::new("relative"),
            "transform.js",
            "typescript",
            "targetRoot must be an absolute path",
        ),
        (
            workflow,
            &missing_root,
            "transform.js",
            "typescript",
            "failed to resolve targetRoot",
        ),
    ];
    for (script_root, target_root, script, language, expected) in cases {
        let error = fixture
            .run_with(script_root, target_root, script, language, &fixture.files)
            .await
            .expect_err(expected);
        assert!(error.contains(expected), "{expected}: {error}");
    }

    let selector = Fixture::new(
        r#"export function getSelector() { throw new Error("selector exploded"); }
export default async function transform() { return null; }"#,
        &[("a.ts", "a();\n")],
    );
    let error = selector.run().await.expect_err("selector error");
    assert!(error.contains("failed to load JSSG selector"), "{error}");
    fixture.assert_untouched();
}

#[tokio::test]
async fn requested_paths_must_stay_inside_the_target_root() {
    let fixture = Fixture::new(REPLACE, &[("src/a.ts", "a();\n")]);
    for bad in [
        "../a.ts",
        "/abs/a.ts",
        "src/../../a.ts",
        "C:\\a.ts",
        "\\\\server\\a.ts",
        "c:/a.ts",
        " ",
    ] {
        let files = [BatchFile {
            path: bad.to_string(),
            content: "a();\n".to_string(),
        }];
        let error = fixture
            .run_with(
                fixture.workflow.path(),
                fixture.root(),
                "transform.js",
                "typescript",
                &files,
            )
            .await
            .expect_err(bad);
        assert!(
            error.contains("safe relative path") || error.contains("must not be empty"),
            "{bad:?}: {error}"
        );
    }
    // A `..` inside a name is an ordinary name; a new file under new
    // directories is fine.
    let files = [BatchFile {
        path: "brand/new/foo..bar.ts".to_string(),
        content: "oldApi('x');\n".to_string(),
    }];
    let outcomes = fixture
        .run_with(
            fixture.workflow.path(),
            fixture.root(),
            "transform.js",
            "typescript",
            &files,
        )
        .await
        .expect("new file");
    assert_eq!(outcomes[0].edits[0].path, "brand/new/foo..bar.ts");
    assert!(!fixture.root().join("brand").exists());
}

#[tokio::test]
async fn produced_paths_outside_the_root_are_rejected() {
    let fixture = Fixture::new(
        r#"import { jssgTransform } from "codemod:ast-grep";
export default async function transform(root, options) {
  const file = root.relativeFilename().replaceAll("\\", "/");
  if (file === "rename.ts") root.rename("../escaped.ts");
  if (file === "secondary.ts") {
    await jssgTransform(async () => "x", options.targetDir + "/../outside.ts", "typescript");
  }
  return "x();\n";
}"#,
        &[("rename.ts", "a();\n"), ("secondary.ts", "b();\n")],
    );
    for file in &fixture.files {
        let error = fixture
            .run_with(
                fixture.workflow.path(),
                fixture.root(),
                "transform.js",
                "typescript",
                std::slice::from_ref(file),
            )
            .await
            .expect_err(&file.path);
        assert!(
            error.contains("outside the target directory"),
            "{}: {error}",
            file.path
        );
    }
    let parent = fixture.root().parent().expect("parent");
    assert!(!parent.join("escaped.ts").exists());
    assert!(!parent.join("outside.ts").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_that_leave_the_root_are_rejected_on_both_directions() {
    let fixture = Fixture::new(
        r#"export default async function transform(root) {
  if (root.relativeFilename().includes("escape")) root.rename("escape/out.ts");
  return root.root().text() + "// touched\n";
}"#,
        &[("escape-me.ts", "a();\n")],
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

    // A symlinked source file resolves outside the root, as does a new file
    // beneath a symlinked directory.
    for path in ["link.ts", "escape/new.ts"] {
        let files = [BatchFile {
            path: path.to_string(),
            content: "secret();\n".to_string(),
        }];
        let error = fixture
            .run_with(
                fixture.workflow.path(),
                fixture.root(),
                "transform.js",
                "typescript",
                &files,
            )
            .await
            .expect_err(path);
        assert!(error.contains("escapes the target root"), "{path}: {error}");
    }
    // A rename into a symlinked directory that points outside is rejected by
    // the sandbox's own check and would be by the bridge's normalization.
    let error = fixture.run().await.expect_err("rename through symlink");
    assert!(
        error.contains("outside the target directory") || error.contains("escapes the target root"),
        "{error}"
    );
    assert!(!outside.path().join("dir/out.ts").exists());
    assert_eq!(read(outside.path(), "secret.ts"), "secret();\n");
}

#[tokio::test]
async fn workspace_semantics_share_one_provider_and_stage_cross_file_writes() {
    let mut fixture = Fixture::new(
        r#"export default async function transform(root) {
  const file = root.relativeFilename().replaceAll("\\", "/");
  const call = root.root().findAll({ rule: { pattern: "add" } })
    .find((node) => node.parent()?.kind() === "call_expression");
  if (!call) return { content: null, output: { file, definition: null } };
  let definition = call.definition();
  for (let hop = 0; definition && definition.root.filename() === root.filename() && hop < 3; hop++) {
    definition = definition.node.definition();
  }
  if (!definition) return { content: null, output: { file, definition: null } };
  definition.root.write(definition.root.root().text().replace("add", "sum"));
  return { content: null, output: { file, definition: definition.root.relativeFilename() } };
}"#,
        &[
            (
                "main.ts",
                "import { add } from \"./utils\";\nconst result = add(1, 2);\n",
            ),
            (
                "utils.ts",
                "export function add(a: number, b: number): number {\n  return a + b;\n}\n",
            ),
        ],
    );
    fixture.semantic = Some(SemanticAnalysis::Mode(SemanticMode::Workspace));

    let outcomes = fixture.run().await.expect("batch");

    assert_eq!(
        outcomes[0].output,
        Some(json!({ "file": "main.ts", "definition": "utils.ts" }))
    );
    assert_eq!(outcomes[0].edits.len(), 1, "{:?}", outcomes[0].edits);
    assert_eq!(outcomes[0].edits[0].path, "utils.ts");
    assert!(outcomes[0].edits[0].content.contains("function sum"));
    assert_eq!(outcomes[0].edits[0].rename_to, None);
    assert_eq!(outcomes[1].edits, vec![]);
    fixture.assert_untouched();

    // A root that requires workspace mode, or does not exist, fails setup.
    for (semantic, expected) in [
        (
            SemanticAnalysis::Detailed(SemanticAnalysisDetails {
                mode: SemanticMode::File,
                root: Some("src".to_string()),
            }),
            "requires workspace mode",
        ),
        (
            SemanticAnalysis::Detailed(SemanticAnalysisDetails {
                mode: SemanticMode::Workspace,
                root: Some("nope".to_string()),
            }),
            "failed to resolve semanticAnalysis.root",
        ),
    ] {
        fixture.semantic = Some(semantic);
        let error = fixture.run().await.expect_err(expected);
        assert!(error.contains(expected), "{expected}: {error}");
    }
}
