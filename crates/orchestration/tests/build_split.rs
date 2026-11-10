//! The `jssg()` split: inline transforms become standalone artifacts
//! (helpers bundled, identity by content hash, no paths), the workflow module
//! is rewritten to reference them, and unsupported captures are refused with
//! a position.
use std::{fs, path::Path};

use orchestration::build::{build_module, BuiltModule};
use regex::Regex;
use tempfile::TempDir;

const HEADER: &str = r#"import { jssg, workflow } from "@codemod.com/orchestration";
import type { SgRoot } from "@codemod.com/jssg-types/main";
import { migrateText, OLD_API } from "./helpers.ts";
import * as helpers from "./helpers.ts";
import transformFn from "./helpers.ts";
"#;

const METHOD_BODY: &str = r#"
const migrate = jssg({
  name: "migrate",
  language: "typescript",
  transform(root: SgRoot, options) {
    const input = options.params.input;
    return { content: migrateText(root.root().text()), output: { input } };
  },
});
export default dynamic(() => migrate());
"#;

fn method() -> String {
    format!("{HEADER}{METHOD_BODY}")
}

/// A checkout with `helpers.ts`, which imports `sub/format.ts`.
fn checkout() -> TempDir {
    let dir = TempDir::new().unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    fs::write(
        dir.path().join("sub/format.ts"),
        "export const banner = (s: string) => `// ${s}\\n`;\n",
    )
    .unwrap();
    fs::write(
        dir.path().join("helpers.ts"),
        r#"import { banner } from "./sub/format.ts";
export const OLD_API = "oldApi";
export function migrateText(text: string): string { return banner("migrated") + text.replaceAll(OLD_API, "newApi"); }
export function unused(): number { return 1; }
export default function transformFn(root: { root(): { text(): string } }) { return migrateText(root.root().text()); }
"#,
    )
    .unwrap();
    dir
}

async fn build_in(dir: &Path, source: &str) -> BuiltModule {
    build_module(source, &dir.join("workflow.ts"))
        .await
        .unwrap()
}

async fn build(source: &str) -> BuiltModule {
    let dir = checkout();
    build_in(dir.path(), source).await
}

async fn method_artifact() -> String {
    build(&method()).await.artifacts.remove(0).source
}

async fn error(body: &str) -> String {
    let dir = checkout();
    let source = format!("{HEADER}\n{body}\n");
    build_module(&source, &dir.path().join("workflow.ts"))
        .await
        .unwrap_err()
        .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_method_transform_becomes_one_artifact_named_after_the_call() {
    let built = build(&method()).await;
    let names: Vec<_> = built.artifacts.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["migrate"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_artifact_is_identified_by_the_sha256_of_its_source() {
    let artifact = build(&method()).await.artifacts.remove(0);
    let expected = {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(artifact.source.as_bytes()))
    };
    assert_eq!(artifact.hash, expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_artifact_bundles_the_helpers_its_transform_reaches_transitively() {
    let source = method_artifact().await;
    assert!(
        source.contains("function migrateText") && source.contains("const banner"),
        "{source}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_artifact_leaves_out_helpers_its_transform_does_not_reach() {
    assert!(!method_artifact().await.contains("unused"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_artifact_leaves_out_the_rest_of_the_workflow_module() {
    let source = method_artifact().await;
    assert!(
        !source.contains("dynamic(") && !source.contains("orchestration"),
        "{source}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_artifact_default_exports_the_transform() {
    let source = method_artifact().await;
    assert!(
        Regex::new(r"export \{\s*transform as default\s*\}")
            .unwrap()
            .is_match(&source),
        "{source}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_artifact_names_no_absolute_path() {
    let dir = checkout();
    let source = build_in(dir.path(), &method())
        .await
        .artifacts
        .remove(0)
        .source;
    let real = fs::canonicalize(dir.path()).unwrap();
    assert!(!source.contains(real.to_str().unwrap()), "{source}");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_call_references_its_artifact_by_name_and_hash() {
    let built = build(&method()).await;
    let reference = format!(
        r#"transform: {{"name":"migrate","hash":"{}"}}"#,
        built.artifacts[0].hash
    );
    assert!(built.source.contains(&reference), "{}", built.source);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_module_keeps_everything_but_the_transform() {
    let source = build(&method()).await.source;
    assert!(
        source.contains("export default dynamic(() => migrate());")
            && !source.contains("transform(root"),
        "{source}"
    );
}

/// `accepts!(test, transform property, what the artifact must contain)`.
macro_rules! accepts {
    ($test:ident, $property:expr, $expected:expr) => {
        #[tokio::test(flavor = "multi_thread")]
        async fn $test() {
            let source = format!(
                "{HEADER}import transform from \"./helpers.ts\";\nexport const t = jssg({{ name: \"t\", language: \"tsx\", {} }});\n",
                $property
            );
            let artifact = build(&source).await.artifacts.remove(0).source;
            assert!(
                Regex::new($expected).unwrap().is_match(&artifact),
                "{artifact}"
            );
        }
    };
}

accepts!(
    accepts_an_arrow_function,
    "transform: (root) => migrateText(root.root().text())",
    r"migrateText\("
);
accepts!(
    accepts_an_async_function_expression,
    "transform: async function (root) { return migrateText(root.root().text()); }",
    r"async function"
);
accepts!(
    accepts_an_imported_binding,
    "transform: transformFn",
    r"function transformFn"
);
accepts!(
    accepts_a_namespace_member_as_a_capture_free_reference,
    "transform: (root) => helpers.migrateText(root.root().text())",
    r"migrateText"
);
accepts!(
    accepts_a_shorthand_imported_binding,
    "transform",
    r"function transformFn"
);

/// Any position (or none) before `message`.
macro_rules! at {
    ($message:expr) => {
        concat!(r"(:\d+:\d+)?: .*", $message)
    };
}

/// `rejects!(test, module body after the header, expected message after the
/// module's path)`.
macro_rules! rejects {
    ($test:ident, $body:expr, $message:expr) => {
        #[tokio::test(flavor = "multi_thread")]
        async fn $test() {
            let message = error($body).await;
            assert!(
                Regex::new(&format!(r"workflow\.ts{}", $message))
                    .unwrap()
                    .is_match(&message),
                "{message}"
            );
        }
    };
}

rejects!(
    rejects_a_captured_top_level_const_at_its_reference,
    r#"const PATTERN = "x";
export const t = jssg({ name: "t", language: "tsx", transform(root) { return root.root().text().replace(PATTERN, ""); } });"#,
    r":8:\d+: jssg 't' transform uses 'PATTERN', declared in the workflow module at line 7; .*import it, or pass it through invocation input"
);
rejects!(
    rejects_a_captured_top_level_function,
    r#"function helper() { return 1; }
export const t = jssg({ name: "t", language: "tsx", transform() { return String(helper()); } });"#,
    at!(r"uses 'helper', declared in the workflow module at line 7")
);
rejects!(
    rejects_a_captured_top_level_class,
    r#"class Box {}
export const t = jssg({ name: "t", language: "tsx", transform() { return String(new Box()); } });"#,
    at!(r"uses 'Box', declared in the workflow module")
);
rejects!(
    rejects_a_capture_in_a_nested_arrow,
    r#"let count = 0;
export const t = jssg({ name: "t", language: "tsx", transform(root) { return [1].map(() => count++).join(); } });"#,
    at!(r"uses 'count'")
);
rejects!(
    rejects_the_orchestration_runtime_inside_a_transform,
    r#"export const t = jssg({ name: "t", language: "tsx", transform() { return String(workflow); } });"#,
    at!(
        r"uses 'workflow' from @codemod\.com/orchestration; the orchestration runtime is not available inside a transform"
    )
);
rejects!(
    rejects_a_computed_name,
    r#"const NAME = "t";
export const t = jssg({ name: NAME, language: "tsx", transform() { return null; } });"#,
    at!(r"jssg name must be a string literal")
);
rejects!(
    rejects_a_missing_transform,
    r#"export const t = jssg({ name: "t", language: "tsx" });"#,
    at!(r"jssg 't' has no transform")
);
rejects!(
    rejects_a_generator_transform,
    r#"export const t = jssg({ name: "t", language: "tsx", *transform() {} });"#,
    at!(r"cannot be a generator")
);
rejects!(
    rejects_a_transform_that_is_not_a_function,
    r#"export const t = jssg({ name: "t", language: "tsx", transform: 42 });"#,
    at!(r"must be a method, a function or arrow expression, or an imported binding")
);
rejects!(
    rejects_options_that_are_not_an_object_literal,
    r#"const options = { name: "t" };
export const t = jssg(options);"#,
    at!(r"jssg\(\) must be called with an object literal")
);
rejects!(
    rejects_an_import_that_does_not_resolve,
    r#"import { nope } from "./missing.ts";
export const t = jssg({ name: "t", language: "tsx", transform() { return nope(); } });"#,
    at!(r"failed to bundle a jssg transform: .*missing\.ts")
);

#[tokio::test(flavor = "multi_thread")]
async fn names_bound_inside_the_transform_property_names_and_globals_are_not_captures() {
    let body = r#"
const PATTERN = "module";
const config = { PATTERN: 1 };
function helper() {}
class Box {}
export const t = jssg({
  name: "t",
  language: "tsx",
  transform(root, { params: { input } }) {
    const PATTERN = "local";
    function helper() { return PATTERN; }
    for (const Box of [1]) console.log(Box);
    try { throw 1; } catch (config) { console.log(config); }
    const named = class helper {};
    label: for (const [helper2] of [[1]]) { if (helper2) break label; }
    const { PATTERN: renamed = undefined } = { PATTERN: input };
    return JSON.stringify({ helper: helper(), config: { PATTERN }.PATTERN, named: typeof named, renamed, input });
  },
});"#;
    let artifact = build(&format!("{HEADER}{body}"))
        .await
        .artifacts
        .remove(0)
        .source;
    assert!(
        artifact.contains("\"local\"") && !artifact.contains("\"module\""),
        "{artifact}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn every_call_in_a_module_gets_its_own_artifact() {
    let body = r#"
export const a = jssg({ name: "a", language: "tsx", transform: () => "a" });
export const b = jssg({ name: "b", language: "typescript", transform: () => "b" });"#;
    let built = build(&format!("{HEADER}{body}")).await;
    let names: Vec<_> = built.artifacts.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, ["a", "b"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn imports_only_transforms_use_are_dropped_from_the_module() {
    let source = build(&format!(
        "{HEADER}\nexport default jssg({{ name: \"imported\", language: \"typescript\", transform: transformFn }});"
    ))
    .await
    .source;
    assert!(!source.contains("import transformFn"), "{source}");
}

#[tokio::test(flavor = "multi_thread")]
async fn imports_the_module_still_uses_are_kept() {
    let source = build(&format!(
        "{HEADER}\nexport default jssg({{ name: \"imported\", language: \"typescript\", transform: transformFn }});\nexport const name = transformFn.name;"
    ))
    .await
    .source;
    assert!(source.contains("import transformFn"), "{source}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_artifact_hash_is_the_same_in_another_checkout() {
    let first = checkout();
    let second = checkout();
    let a = build_in(first.path(), &method()).await.artifacts.remove(0);
    let b = build_in(second.path(), &method()).await.artifacts.remove(0);
    assert_eq!(a.hash, b.hash);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_artifact_hash_changes_with_a_helper_it_bundles() {
    let dir = checkout();
    let before = build_in(dir.path(), &method()).await.artifacts.remove(0);
    fs::write(
        dir.path().join("sub/format.ts"),
        "export const banner = () => '';\n",
    )
    .unwrap();
    let after = build_in(dir.path(), &method()).await.artifacts.remove(0);
    assert_ne!(before.hash, after.hash);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_artifact_hash_changes_with_the_transform_body() {
    let dir = checkout();
    let before = build_in(dir.path(), &method()).await.artifacts.remove(0);
    let changed = method().replace("output: { input }", "output: { input, v: 2 }");
    let after = build_in(dir.path(), &changed).await.artifacts.remove(0);
    assert_ne!(before.hash, after.hash);
}
