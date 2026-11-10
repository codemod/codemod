//! JSSG edge cases (`fixtures/parity/`) through `codemod workflow run -w
//! workflow.ts`: result conversion, path containment, the selector skip,
//! per-file skips and the transform pool, all reaching the Rust batch rules
//! (`crates/orchestration/src/jssg_batch.rs`) the YAML engine shares. The encoding case also runs
//! the YAML engine and requires the same bytes.

mod support;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde_json::{json, Value};
use support::{files, fixture, tree, Scratch};

/// What a run did, with the repository before it.
#[derive(Debug, PartialEq)]
struct Outcome {
    initial: BTreeMap<String, String>,
    tree: BTreeMap<String, String>,
    output: Option<Value>,
    error: Option<String>,
    /// The per-file skips a successful run reported.
    skipped: Vec<String>,
}

#[derive(Default)]
struct Setup {
    files: BTreeMap<String, String>,
    /// Directory symlinks in the repository to a directory outside it.
    outside_links: Vec<&'static str>,
    /// Transform pool threads (`CODEMOD_JSSG_WORKERS`); default 1.
    workers: Option<usize>,
}

fn two() -> BTreeMap<String, String> {
    files(&[("a.ts", "oldApi(1);\n"), ("b.ts", "plain(2);\n")])
}

fn setup(files: BTreeMap<String, String>) -> Setup {
    Setup {
        files,
        ..Setup::default()
    }
}

/// Runs one case and checks what holds for every case: nothing outside the
/// root changes, and a failing run writes nothing.
fn run(setup: &Setup, input: Value) -> Outcome {
    let scratch = Scratch::with(&setup.files);
    let outside = scratch.path("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("keep.ts"), "keep;\n").unwrap();
    for link in &setup.outside_links {
        std::os::unix::fs::symlink(&outside, scratch.target().join(link)).unwrap();
    }
    let initial = scratch.files();
    let mut command = scratch.command(
        &fixture("parity/workflow.ts"),
        &["--input", &input.to_string()],
    );
    command.env(
        "CODEMOD_JSSG_WORKERS",
        setup.workers.unwrap_or(1).to_string(),
    );
    let result = command.output().unwrap();
    let stderr = normalize(&String::from_utf8_lossy(&result.stderr), &scratch.target());
    let outcome = Outcome {
        tree: scratch.files(),
        output: result
            .status
            .success()
            .then(|| serde_json::from_slice(&result.stdout).unwrap()),
        error: (!result.status.success()).then(|| stderr.clone()),
        skipped: stderr
            .lines()
            .filter(|line| line.contains("skipped"))
            .map(|line| line.trim().to_owned())
            .collect(),
        initial,
    };
    assert_eq!(
        tree(&outside),
        files(&[("keep.ts", "keep;\n")]),
        "outside the root: {outcome:#?}"
    );
    if outcome.error.is_some() {
        assert_eq!(outcome.tree, outcome.initial, "a failed run wrote");
    }
    outcome
}

/// The text with this run's repository path replaced and stack frames removed.
fn normalize(text: &str, repo: &Path) -> String {
    text.replace(&repo.display().to_string(), "<REPO>")
        .lines()
        .filter(|line| !line.trim_start().starts_with("at "))
        .collect::<Vec<_>>()
        .join("\n")
}

fn error(setup: &Setup, input: Value) -> String {
    let outcome = run(setup, input);
    match outcome.error {
        Some(message) => message,
        None => panic!("succeeded: {:?}", outcome.output),
    }
}

fn succeeded(setup: &Setup, input: Value) -> Outcome {
    let outcome = run(setup, input);
    assert!(outcome.error.is_none(), "{outcome:#?}");
    outcome
}

#[test]
fn a_changed_string_result_is_written() {
    let outcome = succeeded(
        &setup(two()),
        json!({ "step": "returns", "input": "changed" }),
    );
    assert_eq!(outcome.tree["a.ts"], "newApi(1);\n");
}

macro_rules! results_that_change_nothing {
    ($($name:ident: $input:literal,)*) => {$(
        #[test]
        fn $name() {
            let outcome = succeeded(&setup(two()), json!({ "step": "returns", "input": $input }));
            assert_eq!(outcome.tree, two());
        }
    )*};
}

results_that_change_nothing! {
    an_unchanged_string_result_writes_nothing: "unchanged",
    a_null_result_writes_nothing: "null",
    an_undefined_result_writes_nothing: "undefined",
    a_structured_null_result_writes_nothing: "structured-null",
}

macro_rules! invalid_results_fail_with_the_sandbox_error {
    ($($name:ident: $input:literal => $message:expr,)*) => {$(
        #[test]
        fn $name() {
            let message = error(&setup(two()), json!({ "step": "returns", "input": $input }));
            assert!(message.contains($message), "{message}");
        }
    )*};
}

const NO_OUTPUT: &str = "Object codemod results must contain an 'output' field";

invalid_results_fail_with_the_sandbox_error! {
    an_int_result_fails: "int" => "must return either a string or null/undefined. Received int",
    a_float_result_fails: "float" => "must return either a string or null/undefined. Received float",
    a_bool_result_fails: "bool" => "must return either a string or null/undefined. Received bool",
    an_array_result_fails: "array" => NO_OUTPUT,
    structured_content_that_is_not_a_string_fails: "structured-bad-content" => "must return either a string or null/undefined. Received int",
    a_structured_result_without_output_fails: "missing-output" => NO_OUTPUT,
}

#[test]
fn structured_outputs_convert_to_the_same_json() {
    let outcome = succeeded(
        &setup(two()),
        json!({ "step": "returns", "input": "output-shapes" }),
    );
    let shape = |file: &str| {
        json!({
            "file": file,
            "nested": { "list": [1, "two", null, null, 2.5, -3, [true]] },
            "missing": null,
            "fn": null,
            "big": 1u64 << 40,
            "empty": {},
        })
    };
    assert_eq!(outcome.output, Some(json!([shape("a.ts"), shape("b.ts")])));
}

#[test]
fn a_thrown_error_fails_the_command_and_writes_nothing() {
    let message = error(&setup(two()), json!({ "step": "throwing" }));
    assert!(message.contains("JSSG failed for 'b.ts'"), "{message}");
}

#[test]
fn a_file_whose_transform_throws_is_skipped_and_the_others_are_written() {
    let outcome = succeeded(&setup(two()), json!({ "step": "skipping" }));
    assert_eq!(
        outcome.tree,
        files(&[("a.ts", "newApi(1);\n"), ("b.ts", "plain(2);\n")])
    );
}

#[test]
fn a_skipped_file_is_reported() {
    let outcome = succeeded(&setup(two()), json!({ "step": "skipping" }));
    assert_eq!(
        outcome.skipped,
        ["jssg 'skipping': skipped JSSG failed for 'b.ts': Error: b.ts is not allowed"]
    );
}

fn eighty() -> BTreeMap<String, String> {
    (0..80)
        .map(|n| {
            let name = format!("f{n:02}{}.ts", if n % 9 == 0 { "b" } else { "" });
            let content = if n % 3 == 0 {
                format!("plain({n});\n")
            } else {
                format!("oldApi({n});\n")
            };
            (name, content)
        })
        .collect()
}

macro_rules! the_transform_pool_matches_one_thread {
    ($($name:ident: $step:literal,)*) => {$(
        #[test]
        fn $name() {
            let serial = succeeded(&setup(eighty()), json!({ "step": $step }));
            let pooled = Setup { workers: Some(3), ..setup(eighty()) };
            assert_eq!(succeeded(&pooled, json!({ "step": $step })), serial);
        }
    )*};
}

the_transform_pool_matches_one_thread! {
    a_selected_batch_on_the_pool_matches_one_thread: "selected",
    a_skipping_batch_on_the_pool_matches_one_thread: "skipping",
}

#[test]
fn the_static_selector_skips_files_before_the_transform_runs() {
    let files = files(&[
        ("a.ts", "oldApi(1);\n"),
        ("b.ts", "plain(2);\n"),
        ("c/d.ts", "x(oldApi(3));\n"),
    ]);
    let outcome = succeeded(&setup(files), json!({ "step": "selected" }));
    assert_eq!(outcome.output, Some(json!(["a.ts", "c/d.ts"])));
}

fn multi() -> Outcome {
    let files = files(&[
        ("main.ts", "oldApi(1);\noldApi(2);\nkeep();\n"),
        ("lib/other.ts", "oldApi(3);\n"),
        ("lib/plain.ts", "plain();\n"),
    ]);
    succeeded(
        &setup(files),
        json!({ "step": "multi", "include": ["main.ts"] }),
    )
}

#[test]
fn multiple_edits_a_staged_write_and_a_staged_transform_are_all_written() {
    assert_eq!(
        multi().tree,
        files(&[
            ("lib/other.ts", "newApi(3);\n"),
            ("lib/plain.ts", "export const written = true;\n"),
            ("main.ts", "newApi(1);\nnewApi(2);\nkeep();\n"),
        ])
    );
}

#[test]
fn multiple_edits_report_their_count() {
    assert_eq!(multi().output, Some(json!([2])));
}

#[test]
fn a_rename_inside_the_root_moves_the_file() {
    let outcome = succeeded(
        &setup(two()),
        json!({ "step": "renameInto", "input": "moved", "include": ["a.ts"] }),
    );
    assert_eq!(
        outcome.tree,
        files(&[("b.ts", "plain(2);\n"), ("moved/a.ts", "oldApi(1);\n")])
    );
}

macro_rules! writes_outside_the_root_are_rejected {
    ($($name:ident: $step:literal, $input:literal, $links:expr,)*) => {$(
        #[test]
        fn $name() {
            let case = Setup { outside_links: $links, ..setup(two()) };
            let message = error(&case, json!({ "step": $step, "input": $input, "include": ["a.ts"] }));
            assert!(message.contains("outside the target directory"), "{message}");
        }
    )*};
}

writes_outside_the_root_are_rejected! {
    a_rename_to_the_parent_is_rejected: "renameInto", "..", vec![],
    a_rename_through_a_symlinked_directory_is_rejected: "renameInto", "out", vec!["out"],
    a_secondary_write_through_a_symlinked_directory_is_rejected: "secondaryAt", "out/keep.ts", vec!["out"],
    a_secondary_write_to_the_parent_is_rejected: "secondaryAt", "../escaped.ts", vec![],
}

const BOM: &str = "\u{FEFF}";

/// Files for the `encoding` step, named for what `fixtures/parity/encoding.ts` does with them.
fn encoding_files() -> BTreeMap<String, String> {
    [
        ("edit.ts", "oldApi(1);\n".to_owned()),
        ("bom-edit.ts", format!("{BOM}oldApi(1);\n")),
        (
            "bom-crlf-edit.ts",
            format!("{BOM}oldApi(1);\r\noldApi(2);\r\n"),
        ),
        ("crlf-edit.ts", "oldApi(1);\r\noldApi(2);\r\n".to_owned()),
        ("notrail-edit.ts", "oldApi(1);".to_owned()),
        ("bom-notrail-edit.ts", format!("{BOM}oldApi(1);")),
        ("double-bom-edit.ts", format!("{BOM}{BOM}oldApi(1);\n")),
        ("mid-bom-edit.ts", format!("oldApi(1);\n{BOM}x;\n")),
        ("bom-same.ts", format!("{BOM}keep();\n")),
        ("crlf-same.ts", "keep();\r\n".to_owned()),
        ("bom-rename.ts", format!("{BOM}keep();\n")),
        ("crlf-rename.ts", "keep();\r\n".to_owned()),
        ("bom-null.ts", format!("{BOM}keep();\n")),
    ]
    .into_iter()
    .map(|(path, content)| (path.to_owned(), content))
    .collect()
}

fn encoding() -> BTreeMap<String, String> {
    succeeded(&setup(encoding_files()), json!({ "step": "encoding" })).tree
}

/// What the transform recorded about its input: `/* seen <json> */` at the end.
fn seen(content: &str) -> Value {
    let start = content.find("/* seen ").unwrap() + "/* seen ".len();
    serde_json::from_str(
        content[start..]
            .trim_end()
            .strip_suffix("*/")
            .unwrap()
            .trim(),
    )
    .unwrap()
}

/// The written text before the transform's record.
fn written(content: &str) -> &str {
    &content[..content.find("/* seen ").unwrap_or(content.len())]
}

#[test]
fn the_transform_sees_each_file_verbatim_with_offsets_counting_a_bom() {
    // tree-sitter starts the root after a leading BOM, so `root().text()`
    // lacks it but offsets count it.
    let tree = encoding();
    let seen = |file: &str| seen(&tree[file]);
    let start = |file: &str| seen(file)["root"]["start"].clone();
    let bom = seen("bom-edit.ts");
    assert_eq!(
        json!({
            "edit.ts start": start("edit.ts"),
            "bom-edit.ts": [bom["head"], bom["length"], bom["crlf"], start("bom-edit.ts")],
            "bom-crlf-edit.ts": [seen("bom-crlf-edit.ts")["crlf"], start("bom-crlf-edit.ts")["index"]],
            "double-bom-edit.ts index": start("double-bom-edit.ts")["index"],
        }),
        json!({
            "edit.ts start": { "line": 0, "column": 0, "index": 0 },
            "bom-edit.ts": [[111, 108], 11, false, { "line": 0, "column": 1, "index": 3 }],
            "bom-crlf-edit.ts": [true, 3],
            "double-bom-edit.ts index": 6,
        })
    );
}

#[test]
fn written_bytes_keep_line_endings_a_missing_newline_and_a_bom() {
    let tree = encoding();
    let actual: BTreeMap<&str, Option<&str>> = [
        "crlf-edit.ts",
        "notrail-edit.ts",
        "bom-edit.ts",
        "bom-null.ts",
        "bom-renamed.ts",
        "crlf-renamed.ts",
        "bom-rename.ts",
        "bom-same.ts",
        "crlf-same.ts",
    ]
    .into_iter()
    .map(|file| (file, tree.get(file).map(|content| written(content))))
    .collect();
    let bom_keep = format!("{BOM}keep();\n");
    let bom_edit = format!("{BOM}newApi(1);\n");
    let expected: BTreeMap<&str, Option<&str>> = [
        ("crlf-edit.ts", Some("newApi(1);\r\nnewApi(2);\r\n")),
        ("notrail-edit.ts", Some("newApi(1);")),
        ("bom-edit.ts", Some(bom_edit.as_str())),
        ("bom-null.ts", Some(bom_keep.as_str())),
        ("bom-renamed.ts", Some(bom_keep.as_str())),
        ("crlf-renamed.ts", Some("keep();\r\n")),
        ("bom-rename.ts", None),
        ("bom-same.ts", Some("keep();\n")),
        ("crlf-same.ts", Some("keep();\r\n")),
    ]
    .into_iter()
    .collect();
    assert_eq!(actual, expected);
}

#[test]
fn the_yaml_engine_writes_the_same_bytes() {
    let scratch = Scratch::with(&encoding_files());
    let definition = scratch.path("workflow");
    fs::create_dir_all(&definition).unwrap();
    fs::copy(
        fixture("parity/encoding.ts"),
        definition.join("encoding.ts"),
    )
    .unwrap();
    fs::write(
        definition.join("workflow.yaml"),
        r#"version: "1"
nodes:
  - id: encoding
    name: encoding
    type: automatic
    steps:
      - name: encoding
        js-ast-grep:
          js_file: encoding.ts
          language: typescript
          include: ["**/*.ts"]
"#,
    )
    .unwrap();
    let result = scratch
        .command(
            &definition.join("workflow.yaml"),
            &["--no-interactive", "--allow-dirty"],
        )
        .env("DISABLE_ANALYTICS", "true")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(scratch.files(), encoding());
}
