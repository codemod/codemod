//! One JSSG batch through the real sandbox: the bundled transform runs from
//! memory, outcomes come back as data, the static selector skips files before
//! any runtime starts, one runtime configuration and semantic provider are
//! shared, and paths are validated on both directions of the boundary. Every
//! test uses a temporary repository; nothing on disk changes.

use std::path::{Path, PathBuf};

use butterflow_execution_bridge::{
    jssg::{transform_batch, Batch, Edit, FileOutcome},
    ArtifactRef, BatchFile, Selector, SemanticAnalysis, SemanticAnalysisDetails, SemanticMode,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const REPLACE: &str = r#"export default async function transform(root, options) {
  const text = root.root().text();
  if (text.includes("boom")) throw new Error("boom");
  return {
    content: text.replaceAll("oldApi", "newApi"),
    output: { file: root.relativeFilename().replaceAll("\\", "/"), input: options.params.input ?? null },
  };
}"#;

fn sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

struct Fixture {
    repo: TempDir,
    source: String,
    artifact: ArtifactRef,
    files: Vec<BatchFile>,
    selector: Option<Selector>,
    semantic: Option<SemanticAnalysis>,
    input: Option<Value>,
}

impl Fixture {
    fn new(source: &str, files: &[(&str, &str)]) -> Self {
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
            source: source.to_string(),
            artifact: ArtifactRef {
                name: "t".to_string(),
                hash: sha256(source),
            },
            files,
            selector: None,
            semantic: None,
            input: None,
        }
    }

    fn root(&self) -> &Path {
        self.repo.path()
    }

    fn selector(mut self, rule: Value) -> Self {
        self.selector = Some(Selector {
            rule,
            constraints: None,
            utils: None,
        });
        self
    }

    async fn run_with(
        &self,
        artifact: &ArtifactRef,
        source: Option<&str>,
        target_root: &Path,
        language: &str,
        files: &[BatchFile],
    ) -> Result<Vec<FileOutcome>, String> {
        transform_batch(Batch {
            artifact,
            source,
            language,
            selector: self.selector.as_ref(),
            target_root: target_root.to_str(),
            semantic_analysis: self.semantic.as_ref(),
            input: self.input.as_ref(),
            files,
        })
        .await
    }

    async fn run_files(&self, files: &[BatchFile]) -> Result<Vec<FileOutcome>, String> {
        self.run_with(
            &self.artifact,
            Some(&self.source),
            self.root(),
            "typescript",
            files,
        )
        .await
    }

    async fn run(&self) -> Result<Vec<FileOutcome>, String> {
        self.run_files(&self.files).await
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

fn skipped(path: &str) -> FileOutcome {
    FileOutcome {
        path: path.to_string(),
        edits: vec![],
        output: None,
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
async fn the_bundle_runs_from_memory_with_builtin_imports_only() {
    // No file on disk holds this source; `codemod:ast-grep` still resolves.
    let fixture = Fixture::new(
        r#"import { parse } from "codemod:ast-grep";
var helper = (text) => parse("typescript", text).root().text().toUpperCase();
export default function transform(root) { return helper(root.root().text()); }"#,
        &[("a.ts", "abc;\n")],
    );
    let outcomes = fixture.run().await.expect("batch");
    assert_eq!(outcomes[0].edits, vec![edit("a.ts", "ABC;\n", None)]);
    assert_eq!(outcomes[0].output, None);

    // Sandbox errors name the virtual module derived from the transform name.
    let mut throwing = Fixture::new(
        "export default function transform() { throw new Error('inside'); }",
        &[("a.ts", "a();\n")],
    );
    throwing.artifact.name = "migrate signals/v2".to_string();
    throwing.artifact.hash = sha256(&throwing.source);
    let error = throwing.run().await.expect_err("throws");
    assert!(
        error.contains("inside") && error.contains("migrate_signals_v2.jssg.js"),
        "{error}"
    );
}

#[tokio::test]
async fn static_selectors_skip_files_before_the_sandbox_and_legacy_returns_hold() {
    // The transform throws for any file without `oldApi`, so a skipped file
    // proves the sandbox never ran for it; string | null returns carry no output.
    let fixture = Fixture::new(
        r#"export default async function transform(root) {
  const text = root.root().text();
  if (!text.includes("oldApi")) throw new Error("ran on a non-matching file");
  return text.includes("skip") ? null : text.replaceAll("oldApi", "newApi");
}"#,
        &[
            ("a.ts", "oldApi('a');\n"),
            ("skip.ts", "oldApi('skip');\n"),
            ("other.ts", "other();\n"),
        ],
    )
    .selector(json!({ "pattern": "oldApi($A)" }));

    let outcomes = fixture.run().await.expect("batch");
    assert_eq!(
        outcomes,
        vec![
            FileOutcome {
                path: "a.ts".to_string(),
                edits: vec![edit("a.ts", "newApi('a');\n", None)],
                output: None,
            },
            skipped("skip.ts"),
            skipped("other.ts"),
        ]
    );
    fixture.assert_untouched();

    // Without a selector every file runs, so the non-matching file throws.
    let mut unfiltered = Fixture::new(&fixture.source, &[("other.ts", "other();\n")]);
    unfiltered.selector = None;
    let error = unfiltered.run().await.expect_err("transform ran");
    assert!(error.contains("ran on a non-matching file"), "{error}");

    // Constraints apply; an invalid rule fails setup before any file runs.
    let constrained = Fixture::new(REPLACE, &[("a.ts", "oldApi(a);\n")]).selector(json!({}));
    let mut constrained = constrained;
    constrained.selector = Some(Selector {
        rule: json!({ "pattern": "oldApi($A)" }),
        constraints: Some(json!({ "A": { "kind": "string" } })),
        utils: None,
    });
    assert_eq!(
        constrained.run().await.expect("batch"),
        vec![skipped("a.ts")]
    );
    let invalid = Fixture::new(REPLACE, &[("a.ts", "oldApi('a');\n")])
        .selector(json!({ "nope": "oldApi($A)" }));
    let error = invalid.run().await.expect_err("invalid rule");
    assert!(error.contains("invalid JSSG selector"), "{error}");
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

    let root = fixture.root();
    let missing_root = root.join("nope");
    let artifact = |name: &str, hash: &str| ArtifactRef {
        name: name.to_string(),
        hash: hash.to_string(),
    };
    let good = &fixture.artifact;
    let source = fixture.source.as_str();
    let cases: Vec<(ArtifactRef, Option<&str>, &Path, &str, &str)> = vec![
        // (artifact, source, target root, language, expected error)
        (
            good.clone(),
            Some(source),
            root,
            "klingon",
            "invalid JSSG language",
        ),
        (
            good.clone(),
            None,
            root,
            "typescript",
            "has no artifact source",
        ),
        (
            good.clone(),
            Some("export default () => null;"),
            root,
            "typescript",
            "not the recorded",
        ),
        (
            artifact("t", "abc"),
            Some(source),
            root,
            "typescript",
            "must be lowercase hex SHA-256",
        ),
        (
            artifact("t", &good.hash.to_uppercase()),
            Some(source),
            root,
            "typescript",
            "must be lowercase hex SHA-256",
        ),
        (
            artifact(" ", &good.hash),
            Some(source),
            root,
            "typescript",
            "name must not be empty",
        ),
        (
            good.clone(),
            Some(source),
            Path::new("relative"),
            "typescript",
            "targetRoot must be an absolute path",
        ),
        (
            good.clone(),
            Some(source),
            &missing_root,
            "typescript",
            "failed to resolve targetRoot",
        ),
    ];
    for (artifact, source, target_root, language, expected) in cases {
        let error = fixture
            .run_with(&artifact, source, target_root, language, &fixture.files)
            .await
            .expect_err(expected);
        assert!(error.contains(expected), "{expected}: {error}");
    }
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
        let error = fixture.run_files(&files).await.expect_err(bad);
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
    let outcomes = fixture.run_files(&files).await.expect("new file");
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
            .run_files(std::slice::from_ref(file))
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
        let error = fixture.run_files(&files).await.expect_err(path);
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
async fn workspace_semantics_index_every_selected_file_and_stage_cross_file_writes() {
    // The selector matches only `main.ts`; `utils.ts` is skipped but must
    // still be indexed so the definition of `add` resolves and its file can
    // be edited through the provider.
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
    )
    .selector(json!({ "pattern": "add($A, $B)" }));
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
    assert_eq!(outcomes[1], skipped("utils.ts"));
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
