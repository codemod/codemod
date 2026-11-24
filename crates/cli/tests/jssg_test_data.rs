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

/// The data mismatch is reported on the case's last file; a filter that
/// leaves that file out must not hide it.
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
