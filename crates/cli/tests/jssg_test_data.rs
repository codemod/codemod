//! `codemod jssg test` snapshots a directory case's returned data in one
//! `data.json` at the case's root, never among its output files.

use std::fs;
use std::path::Path;
use std::process::Command;

const TRANSFORM: &str = r#"
export default function transform(root: any) {
  const text = root.root().text();
  return { edit: text.replace("OLD", "NEW"), data: { kind: "seen" } };
}
"#;

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn jssg_test(dir: &Path) -> std::process::Output {
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
        .output()
        .unwrap()
}

#[test]
fn a_directory_case_with_nested_files_snapshots_data_at_its_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write(&root.join("scripts/codemod.ts"), TRANSFORM);
    for name in ["src/a.ts", "src/lib/b.ts", "c.ts"] {
        write(
            &root.join("tests/case/input").join(name),
            "const a = 'OLD';\n",
        );
        write(
            &root.join("tests/case/expected").join(name),
            "const a = 'NEW';\n",
        );
    }

    let first = jssg_test(root);
    let output = |run: &std::process::Output| {
        format!(
            "{}{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        )
    };
    assert!(first.status.success(), "{}", output(&first));

    let case = root.join("tests/case");
    let data: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(case.join("data.json")).unwrap()).unwrap();
    assert_eq!(
        data,
        serde_json::json!([
            { "file": "c.ts", "data": { "kind": "seen" } },
            { "file": "src/a.ts", "data": { "kind": "seen" } },
            { "file": "src/lib/b.ts", "data": { "kind": "seen" } },
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
    let second = jssg_test(root);
    assert!(second.status.success(), "{}", output(&second));
}
