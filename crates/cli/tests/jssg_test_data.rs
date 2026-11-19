use std::fs;
use std::path::Path;
use std::process::Command;

fn write(dir: &Path, path: &str, text: &str) {
    let full = dir.join(path);
    fs::create_dir_all(full.parent().unwrap()).unwrap();
    fs::write(full, text).unwrap();
}

#[test]
fn a_directory_case_with_nested_files_keeps_one_data_json_next_to_input() {
    let package = tempfile::tempdir().unwrap();
    write(
        package.path(),
        "transform.ts",
        "export default function transform(root: any) { return { edit: null, data: 1 }; }\n",
    );
    for side in ["input", "expected"] {
        write(
            package.path(),
            &format!("tests/nested/{side}/src/a.ts"),
            "a;\n",
        );
        write(package.path(), &format!("tests/nested/{side}/b.ts"), "b;\n");
    }

    let output = Command::new(env!("CARGO_BIN_EXE_codemod"))
        .args(["jssg", "test", "transform.ts", "--language", "typescript"])
        .current_dir(package.path())
        .env("DISABLE_ANALYTICS", "true")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let data = fs::read_to_string(package.path().join("tests/nested/data.json")).unwrap();
    let data: serde_json::Value = serde_json::from_str(&data).unwrap();
    assert_eq!(
        data,
        serde_json::json!([{ "file": "b.ts", "data": 1 }, { "file": "src/a.ts", "data": 1 }])
    );
}
