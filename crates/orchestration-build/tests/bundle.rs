//! The QuickJS bundle: one self-contained module, no Node, async lowered,
//! carrying exactly the artifacts the split produces.
use std::{
    fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use orchestration_build::{build_file, build_quickjs_bundle, QuickjsBundle, Runtime};
use regex::Regex;
use tempfile::TempDir;
use tokio::sync::OnceCell;

fn package() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/orchestration")
}

fn runtime() -> Runtime {
    Runtime {
        src: fs::canonicalize(package().join("src")).unwrap(),
    }
}

fn fixture(path: &str) -> PathBuf {
    fs::canonicalize(package().join("tests/fixtures").join(path)).unwrap()
}

/// The `jssg-sequence` fixture bundle, built once for this file.
async fn sequence() -> &'static QuickjsBundle {
    static BUNDLE: OnceCell<QuickjsBundle> = OnceCell::const_new();
    BUNDLE
        .get_or_init(|| async {
            build_quickjs_bundle(&fixture("quickjs/jssg-sequence.ts"), &runtime())
                .await
                .unwrap()
        })
        .await
}

/// The bundle after its first line (the artifact table), without comments
/// and strings.
fn workflow_code(bundle: &QuickjsBundle) -> String {
    static LITERALS: OnceLock<Regex> = OnceLock::new();
    let body = &bundle.code[bundle.code.find('\n').unwrap() + 1..];
    LITERALS
        .get_or_init(|| {
            Regex::new(r#"(?m)/\*[\s\S]*?\*/|`(?:[^`\\]|\\.)*`|"(?:[^"\\\n]|\\.)*"|'(?:[^'\\\n]|\\.)*'|//.*$"#)
                .unwrap()
        })
        .replace_all(body, "\"\"")
        .into_owned()
}

async fn error(workflow: &str) -> String {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("workflow.ts");
    fs::write(&file, workflow).unwrap();
    build_quickjs_bundle(&file, &runtime())
        .await
        .unwrap_err()
        .to_string()
}

fn index() -> String {
    serde_json::to_string(&runtime().src.join("index.ts")).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bundle_has_no_imports() {
    let bundle = sequence().await;
    assert!(
        !Regex::new(r"(?m)^\s*import\s")
            .unwrap()
            .is_match(&bundle.code),
        "{}",
        &bundle.code[..2000.min(bundle.code.len())]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bundle_lowers_every_async_function_and_await() {
    let code = workflow_code(sequence().await);
    let remaining: Vec<_> = Regex::new(r"\basync\b|\bawait\b")
        .unwrap()
        .find_iter(&code)
        .map(|m| &code[m.start().saturating_sub(60)..(m.end() + 60).min(code.len())])
        .collect();
    assert_eq!(remaining, Vec::<&str>::new());
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bundle_exports_main() {
    let code = &sequence().await.code;
    assert!(Regex::new(r"export\s*\{[^}]*\bmain\b")
        .unwrap()
        .is_match(code));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bundle_hash_is_the_sha256_of_its_code() {
    let bundle = sequence().await;
    let expected = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(bundle.code.as_bytes()))
    };
    assert_eq!(bundle.hash, expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bundle_carries_the_artifacts_the_split_produces() {
    let bundle = sequence().await;
    let split = build_file(&fixture("jssg/transforms.ts")).await.unwrap();
    let names_and_hashes = |artifacts: &[orchestration_build::JssgArtifact]| {
        let mut pairs: Vec<_> = artifacts
            .iter()
            .map(|a| (a.name.clone(), a.hash.clone()))
            .collect();
        pairs.sort();
        pairs
    };
    assert_eq!(
        names_and_hashes(&bundle.artifacts),
        names_and_hashes(&split.artifacts)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bundle_lists_helpers_bundled_only_into_artifacts_as_inputs() {
    let bundle = sequence().await;
    assert!(bundle.inputs.contains(&fixture("jssg/helpers.ts")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deep_package_import_is_rejected() {
    let message =
        error("import { run } from \"@codemod.com/orchestration/runtime\";\nexport default run;\n")
            .await;
    assert!(
        message.contains("'@codemod.com/orchestration/runtime' is not available in the quickjs runtime; import '@codemod.com/orchestration' (the portable API)"),
        "{message}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_package_name_resolves_to_the_portable_runtime() {
    let dir = TempDir::new().unwrap();
    let file = dir.path().join("workflow.ts");
    fs::write(
        &file,
        "import { shell } from \"@codemod.com/orchestration\";\nexport default shell({ name: \"x\", command: \"true\" });\n",
    )
    .unwrap();
    let bundle = build_quickjs_bundle(&file, &runtime()).await.unwrap();
    assert!(bundle.inputs.contains(&runtime().src.join("portable.ts")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_split_error_in_the_workflow_graph_keeps_its_position() {
    let message = error(&format!(
        "import {{ jssg }} from {};\nconst X = 1;\nexport default jssg({{ name: \"t\", language: \"tsx\", transform() {{ return String(X); }} }});\n",
        index()
    ))
    .await;
    assert!(
        Regex::new(r"workflow\.ts:3:\d+: jssg 't' transform uses 'X'")
            .unwrap()
            .is_match(&message),
        "{message}"
    );
}
