//! `codemod jssg test --callback`: fixture tests for a forEachFile callback
//! file, on the real binary.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

const MIGRATE: &str = r#"
import { parse } from "codemod/ast-grep";

export default function migrate(file: any) {
  const root = parse(file).root();
  const found = root.findAll({ rule: { pattern: "var $A = $B" } });
  if (!found.length) return;
  file.write(root.commitEdits(found.map((node: any) => node.replace(node.text().replace(/^var/, "let")))));
  return { path: file.path, count: found.length };
}
"#;

fn write(dir: &Path, path: &str, text: &str) {
    let full = dir.join(path);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(full, text).unwrap();
}

/// A package with `migrate.ts` and one single-file case, `tests/basic`.
fn package() -> TempDir {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "migrate.ts", MIGRATE);
    write(
        dir.path(),
        "tests/basic/input.ts",
        "var a = 1;\nvar b = 2;\n",
    );
    write(
        dir.path(),
        "tests/basic/expected.ts",
        "let a = 1;\nlet b = 2;\n",
    );
    dir
}

fn jssg_test(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_codemod"))
        .current_dir(dir)
        .args(["jssg", "test", "-l", "typescript"])
        .args(args)
        .env("DISABLE_ANALYTICS", "true")
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run codemod")
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn json(path: &Path) -> serde_json::Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn a_callback_passes_its_fixtures_and_its_return_value_is_snapshotted() {
    let dir = package();
    write(dir.path(), "tests/untouched/input.ts", "const c = 3;\n");
    write(dir.path(), "tests/untouched/expected.ts", "const c = 3;\n");

    let output = jssg_test(dir.path(), &["--callback", "migrate.ts"]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(
        json(&dir.path().join("tests/basic/data.json")),
        serde_json::json!([{ "file": "input.ts", "data": { "path": "input.ts", "count": 2 } }])
    );
    // Returning nothing leaves no data.json.
    assert!(!dir.path().join("tests/untouched/data.json").exists());
}

#[test]
fn a_changed_return_value_fails_until_updated() {
    let dir = package();
    write(
        dir.path(),
        "tests/basic/data.json",
        r#"[{ "file": "input.ts", "data": { "path": "input.ts", "count": 3 } }]"#,
    );
    let output = jssg_test(dir.path(), &["--callback", "migrate.ts"]);
    assert!(!output.status.success());
    assert!(text(&output).contains("Data mismatch"), "{}", text(&output));

    let output = jssg_test(dir.path(), &["--callback", "-u", "migrate.ts"]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(
        json(&dir.path().join("tests/basic/data.json"))[0]["data"]["count"],
        2
    );
}

#[test]
fn a_wrong_output_fails_with_a_diff() {
    let dir = package();
    write(dir.path(), "tests/basic/expected.ts", "const a = 1;\n");
    let output = jssg_test(dir.path(), &["--callback", "migrate.ts"]);
    assert!(!output.status.success());
    assert!(text(&output).contains("let a = 1;"), "{}", text(&output));
}

#[test]
fn a_directory_case_runs_the_callback_on_each_file() {
    let dir = package();
    write(dir.path(), "tests/tree/input/a.ts", "var a = 1;\n");
    write(dir.path(), "tests/tree/input/b.ts", "const b = 2;\n");
    write(dir.path(), "tests/tree/input/src/c.ts", "var c = 3;\n");
    write(dir.path(), "tests/tree/expected/a.ts", "let a = 1;\n");
    write(dir.path(), "tests/tree/expected/b.ts", "const b = 2;\n");
    write(dir.path(), "tests/tree/expected/src/c.ts", "let c = 3;\n");
    let output = jssg_test(dir.path(), &["--callback", "migrate.ts"]);
    assert!(output.status.success(), "{}", text(&output));
    // One data.json at the case's root, with the data of files in subfolders.
    assert_eq!(
        json(&dir.path().join("tests/tree/data.json")),
        serde_json::json!([
            { "file": "a.ts", "data": { "path": "a.ts", "count": 1 } },
            { "file": "src/c.ts", "data": { "path": "src/c.ts", "count": 1 } },
        ])
    );
    for stray in ["input/data.json", "input/src/data.json"] {
        assert!(
            !dir.path().join("tests/tree").join(stray).exists(),
            "{stray} was written"
        );
    }
}

#[test]
fn semantic_analysis_resolves_definitions_in_a_callback() {
    let dir = TempDir::new().unwrap();
    write(
        dir.path(),
        "find.ts",
        r#"
import { parse, symbols } from "codemod/ast-grep";
export default (file: any) => {
  const root = parse(file).root();
  const use = root.find({ rule: { pattern: "console.log($X)" } }).getMatch("X");
  const [def] = symbols("typescript").definition.get(use) ?? [];
  return { definedOnLine: def?.node.range().start.line ?? null };
};
"#,
    );
    write(
        dir.path(),
        "tests/def/input.ts",
        "const a = 1;\nconsole.log(a);\n",
    );
    write(
        dir.path(),
        "tests/def/expected.ts",
        "const a = 1;\nconsole.log(a);\n",
    );
    let output = jssg_test(dir.path(), &["--callback", "find.ts"]);
    assert!(output.status.success(), "{}", text(&output));
    assert_eq!(
        json(&dir.path().join("tests/def/data.json"))[0]["data"]["definedOnLine"],
        0
    );
}

#[test]
fn a_callback_that_throws_fails_its_case() {
    let dir = package();
    write(
        dir.path(),
        "migrate.ts",
        "export default () => { throw new Error('nope'); };\n",
    );
    let output = jssg_test(dir.path(), &["--callback", "migrate.ts"]);
    assert!(!output.status.success());
    assert!(text(&output).contains("nope"), "{}", text(&output));
}

#[test]
fn a_file_whose_default_export_is_not_a_function_is_refused() {
    let dir = package();
    write(dir.path(), "migrate.ts", "export default 42;\n");
    let output = jssg_test(dir.path(), &["--callback", "migrate.ts"]);
    assert!(!output.status.success());
    assert!(
        text(&output).contains("must default-export a forEachFile callback"),
        "{}",
        text(&output)
    );
}

#[test]
fn params_are_refused_for_a_callback() {
    let dir = package();
    write(
        dir.path(),
        "test.config.json",
        r#"{ "params": { "strict": true } }"#,
    );
    let output = jssg_test(dir.path(), &["--callback", "migrate.ts"]);
    assert!(!output.status.success());
    assert!(
        text(&output).contains("params are not given to a forEachFile callback"),
        "{}",
        text(&output)
    );
}

#[test]
fn a_callback_run_without_the_flag_says_to_pass_it() {
    let dir = package();
    let output = jssg_test(dir.path(), &["migrate.ts"]);
    assert!(!output.status.success());
    assert!(text(&output).contains("--callback"), "{}", text(&output));
}
