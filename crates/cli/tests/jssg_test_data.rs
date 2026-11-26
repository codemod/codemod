//! `codemod jssg test` snapshots a directory case's returned data in one
//! `data.json` at the case's root, never among its output files.

use std::fs;
use std::path::Path;
use std::process::Command;

const TRANSFORM: &str = r#"
export default function transform(root: any) {
  const text = root.root().text();
  const marker = text.match(/marker: (\w+)/)?.[1];
  return { edit: text.replace("OLD", "NEW"), data: { marker } };
}
"#;

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn jssg_test(dir: &Path, extra: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_codemod"))
        .current_dir(dir)
        .args([
            "--disable-analytics",
            "jssg",
            "test",
            "-l",
            "typescript",
            "scripts/codemod.ts",
            "tests",
        ])
        .args(extra)
        .output()
        .unwrap()
}

fn output(run: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    )
}

#[test]
fn jssg_test_rejects_malformed_fixture_input_even_when_the_snapshot_matches() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        &root.join("scripts/codemod.ts"),
        "export default () => null;",
    );
    write(&root.join("tests/basic/input.ts"), "foo({;\n");
    write(&root.join("tests/basic/expected.ts"), "foo({;\n");
    let run = jssg_test(root, &[]);
    let shown = output(&run);
    assert_eq!(run.status.code(), Some(1), "{shown}");
    assert!(
        shown.contains("input.ts") && shown.contains("Cannot parse"),
        "{shown}"
    );
}

#[test]
fn jssg_test_names_a_malformed_transform_module() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(
        &root.join("scripts/codemod.ts"),
        r#"export { default } from "./broken.js";"#,
    );
    write(
        &root.join("scripts/broken.js"),
        "export default function transform( {;",
    );
    write(&root.join("tests/basic/input.ts"), "foo();\n");
    write(&root.join("tests/basic/expected.ts"), "foo();\n");
    let run = jssg_test(root, &[]);
    let shown = output(&run);
    assert_eq!(run.status.code(), Some(1), "{shown}");
    assert!(
        shown.contains("broken.js") && shown.contains("Parse error"),
        "{shown}"
    );
}

/// A case with `c.ts`, `src/a.ts` and `src/lib/b.ts`, each returning its
/// own marker as data.
fn nested_case(root: &Path) {
    write(&root.join("scripts/codemod.ts"), TRANSFORM);
    for (name, marker) in [("src/a.ts", "a"), ("src/lib/b.ts", "b"), ("c.ts", "c")] {
        write(
            &root.join("tests/case/input").join(name),
            &format!("const a = 'OLD'; // marker: {marker}\n"),
        );
        write(
            &root.join("tests/case/expected").join(name),
            &format!("const a = 'NEW'; // marker: {marker}\n"),
        );
    }
}

/// The right files, but one file's data differs.
const STALE: &str = r#"[
  { "file": "c.ts", "data": { "marker": "other" } },
  { "file": "src/a.ts", "data": { "marker": "a" } },
  { "file": "src/lib/b.ts", "data": { "marker": "b" } }
]"#;

#[test]
fn a_directory_case_with_nested_files_snapshots_data_at_its_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    nested_case(root);

    let first = jssg_test(root, &[]);
    assert!(first.status.success(), "{}", output(&first));

    let case = root.join("tests/case");
    let data: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(case.join("data.json")).unwrap()).unwrap();
    assert_eq!(
        data,
        serde_json::json!([
            { "file": "c.ts", "data": { "marker": "c" } },
            { "file": "src/a.ts", "data": { "marker": "a" } },
            { "file": "src/lib/b.ts", "data": { "marker": "b" } },
        ])
    );
    for stray in [
        "input/data.json",
        "input/src/data.json",
        "input/src/lib/data.json",
        "expected/data.json",
        "expected/src/data.json",
    ] {
        assert!(!case.join(stray).exists(), "{stray} was written");
    }

    // The snapshot now exists and matches.
    let second = jssg_test(root, &[]);
    assert!(second.status.success(), "{}", output(&second));

    // A different snapshot fails the case and is left as it was.
    fs::write(case.join("data.json"), STALE).unwrap();
    let third = jssg_test(root, &[]);
    assert!(!third.status.success(), "{}", output(&third));
    assert!(
        output(&third).contains("Data mismatch"),
        "{}",
        output(&third)
    );
    assert_eq!(fs::read_to_string(case.join("data.json")).unwrap(), STALE);
}

/// The data mismatch is the case's own result; a filter that leaves the
/// case's files out must not hide it.
#[test]
fn a_filter_does_not_hide_a_data_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    nested_case(root);
    let case = root.join("tests/case");
    write(&case.join("data.json"), STALE);

    let run = jssg_test(root, &["--filter", "c.ts"]);
    assert!(!run.status.success(), "{}", output(&run));
    assert!(output(&run).contains("Data mismatch"), "{}", output(&run));
    assert_eq!(fs::read_to_string(case.join("data.json")).unwrap(), STALE);
}

/// A file that an earlier file deleted is skipped by the runner; the case's
/// data is still checked, with the files that ran.
#[test]
fn a_case_whose_last_file_was_deleted_still_checks_its_data() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    nested_case(root);
    write(
        &root.join("scripts/codemod.ts"),
        r#"
import fs from "fs";
export default function transform(root: any, options: any) {
  const text = root.root().text();
  const marker = text.match(/marker: (\w+)/)?.[1];
  // c.ts runs first and deletes the case's last file.
  if (marker === "c") fs.rmSync(`${options.targetDir}/src/lib/b.ts`);
  return { edit: text.replace("OLD", "NEW"), data: { marker } };
}
"#,
    );
    let case = root.join("tests/case");
    fs::remove_file(case.join("expected/src/lib/b.ts")).unwrap();

    let first = jssg_test(root, &["--allow-fs"]);
    assert!(first.status.success(), "{}", output(&first));
    let data: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(case.join("data.json")).unwrap()).unwrap();
    assert_eq!(
        data,
        serde_json::json!([
            { "file": "c.ts", "data": { "marker": "c" } },
            { "file": "src/a.ts", "data": { "marker": "a" } },
        ])
    );

    let stale = r#"[
  { "file": "c.ts", "data": { "marker": "c" } },
  { "file": "src/a.ts", "data": { "marker": "other" } }
]"#;
    fs::write(case.join("data.json"), stale).unwrap();
    let second = jssg_test(root, &["--allow-fs"]);
    assert!(!second.status.success(), "{}", output(&second));
    assert!(
        output(&second).contains("Data mismatch"),
        "{}",
        output(&second)
    );
    assert_eq!(fs::read_to_string(case.join("data.json")).unwrap(), stale);
}

/// The transform that deletes the case's last file (src/lib/b.ts) when it
/// runs on c.ts, its first.
const DELETES_LAST_FILE: &str = r#"
import fs from "fs";
export default function transform(root: any, options: any) {
  const text = root.root().text();
  const marker = text.match(/marker: (\w+)/)?.[1];
  if (marker === "c") fs.rmSync(`${options.targetDir}/src/lib/b.ts`, { force: true });
  return { edit: text.replace("OLD", "NEW"), data: { marker } };
}
"#;

/// The failed tests in a `--reporter json` run, as `(name, output)`.
fn json_failures(run: &std::process::Output) -> Vec<(String, String)> {
    String::from_utf8_lossy(&run.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| event["type"] == "test" && event["event"] == "failed")
        .map(|event| {
            (
                event["name"].as_str().unwrap_or_default().to_string(),
                event["stdout"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// A data mismatch is its case's own result: reported once, also when the
/// filter matches the case's last file.
#[test]
fn a_data_mismatch_is_reported_once() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    nested_case(root);
    write(&root.join("tests/case/data.json"), STALE);

    for filter in ["b.ts", "c.ts"] {
        let run = jssg_test(root, &["--filter", filter]);
        assert!(!run.status.success(), "{}", output(&run));
        assert_eq!(
            output(&run).matches("Data mismatch").count(),
            1,
            "--filter {filter}: {}",
            output(&run)
        );
    }
}

/// The JSON report has the data mismatch of a case whose files the filter
/// left out, and of a case whose last file was deleted.
#[test]
fn the_json_report_has_every_data_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    nested_case(root);
    write(&root.join("tests/case/data.json"), STALE);

    let run = jssg_test(root, &["--filter", "c.ts", "--reporter", "json"]);
    assert!(!run.status.success(), "{}", output(&run));
    let failures = json_failures(&run);
    assert_eq!(failures.len(), 1, "{}", output(&run));
    assert!(failures[0].0.contains("data.json"), "{failures:?}");
    assert!(failures[0].1.contains("Data mismatch"), "{failures:?}");

    write(&root.join("scripts/codemod.ts"), DELETES_LAST_FILE);
    fs::remove_file(root.join("tests/case/expected/src/lib/b.ts")).unwrap();
    write(
        &root.join("tests/case/data.json"),
        r#"[{ "file": "c.ts", "data": { "marker": "other" } }]"#,
    );
    let run = jssg_test(root, &["--allow-fs", "--reporter", "json"]);
    assert!(!run.status.success(), "{}", output(&run));
    let failures = json_failures(&run);
    assert_eq!(failures.len(), 1, "{}", output(&run));
    assert!(failures[0].1.contains("Data mismatch"), "{failures:?}");
}

/// With --fail-fast, a case whose last file was deleted is checked when the
/// case ends, so the run stops there.
#[test]
fn fail_fast_stops_at_a_data_mismatch_when_the_case_ends() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    nested_case(root);
    write(&root.join("scripts/codemod.ts"), DELETES_LAST_FILE);
    fs::remove_file(root.join("tests/case/expected/src/lib/b.ts")).unwrap();
    write(
        &root.join("tests/case/data.json"),
        r#"[{ "file": "c.ts", "data": { "marker": "other" } }]"#,
    );
    // A later case that must not run.
    write(
        &root.join("tests/zlater/input/z.ts"),
        "const z = 'OLD'; // marker: z\n",
    );
    write(
        &root.join("tests/zlater/expected/z.ts"),
        "const z = 'NEW'; // marker: z\n",
    );

    let run = jssg_test(root, &["--allow-fs", "--fail-fast", "--sequential"]);
    assert!(!run.status.success(), "{}", output(&run));
    assert_eq!(
        output(&run).matches("Data mismatch").count(),
        1,
        "{}",
        output(&run)
    );
    assert!(!output(&run).contains("zlater"), "{}", output(&run));
    assert!(!root.join("tests/zlater/data.json").exists());
}
